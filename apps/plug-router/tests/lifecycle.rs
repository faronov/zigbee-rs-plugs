use core::future::Future;
use core::task::{Context, Poll, Waker};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use plug_router_app::{
    APP_STATE_CHECKPOINT_INTERVAL_MS, AlwaysOnEndDevicePlugApp, AppStateCheckpoint,
    CheckpointOutcome, LocalControl, LocalRelaySelection, MeterFault, MeterHealthPolicy,
    MeterSafetyState, MeterService, MeterServiceOutcome, PlugClock, PlugRouterApp, PlugRouterError,
    PlugStatus, RelayCommand, network_status_for_router,
};
use router_app::{
    AlwaysOnEndDeviceApp, NoDiagnostics, NoSupervisor, NodeArchetype, ParentRouterApp,
    PersistentChildren, RouterParts, RouterPolicy, RouterStatus,
};
use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_aps::frames::{ApsDeliveryMode, ApsFrameControl, ApsFrameType, ApsHeader};
use zigbee_mac::PlatformServices;
use zigbee_mac::mock::MockMac;
use zigbee_mac::primitives::{MacFrame, McpsDataIndication};
use zigbee_nwk::DeviceType;
use zigbee_nwk::frames::{NwkFrameControl, NwkFrameType, NwkHeader};
use zigbee_nwk::security::{NwkSecurity, NwkSecurityHeader};
use zigbee_plug_controller::{
    ButtonGesture, ButtonGestureEvent, NetworkStatus, PlugController, status_led_on,
};
use zigbee_plug_core::{ElectricalSample, PlugSettings, PlugStateRecord, ProtectionConfig};
use zigbee_plug_profile::ZigbeePlug;
use zigbee_runtime::child_store::{
    ChildStoreError, ChildTableStore, PersistentChild, PersistentChildTable, RamChildTableStore,
};
use zigbee_runtime::event_loop::StackEvent;
use zigbee_runtime::node::ZigbeeNode;
use zigbee_runtime::nv_storage::{NvError, NvItemId, NvStorage, RamNvStorage};
use zigbee_runtime::power::PowerMode;
use zigbee_runtime::profile::{ApplicationProfile, DeviceProfile, SmartPlugReporting};
use zigbee_runtime::role::{EndDevice, Router};
use zigbee_runtime::security_store::{
    PersistentSecurityState, RamSecurityStateStore, SecurityStateStore, SecurityStoreError,
};
use zigbee_runtime::{UserAction, ZigbeeDevice};
use zigbee_types::{MacAddress, PanId, ShortAddress};
use zigbee_zcl::ClusterDirection;
use zigbee_zcl::DeviceId;
use zigbee_zcl::clusters::Cluster;
use zigbee_zcl::clusters::basic::CMD_RESET_TO_FACTORY_DEFAULTS;
use zigbee_zcl::clusters::on_off::CMD_ON_WITH_TIMED_OFF;
use zigbee_zcl::frame::ZclFrame;

const LOCAL_IEEE: [u8; 8] = [0x11; 8];
const COORDINATOR_IEEE: [u8; 8] = [0xCC; 8];
const EXTENDED_PAN_ID: [u8; 8] = [0xBB; 8];
const NETWORK_KEY: [u8; 16] = [0x42; 16];
const PAN_ID: u16 = 0x1234;
const SHORT_ADDRESS: u16 = 0x5678;
const CHANNEL: u8 = 15;

static POLICY: RouterPolicy = RouterPolicy::DEFAULT;

type TestProfile = DeviceProfile<ZigbeePlug>;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

fn profile() -> TestProfile {
    ZigbeePlug::new(SmartPlugReporting::default())
        .unwrap()
        .into_device_profile(1)
}

fn device(profile: &mut TestProfile) -> ZigbeeDevice<MockMac, Router> {
    let mut mac = MockMac::new(LOCAL_IEEE);
    mac.set_rx_delay_us(u32::MAX);
    ZigbeeDevice::builder(mac)
        .power_mode(PowerMode::AlwaysOn)
        .endpoint(
            profile.endpoint(),
            PROFILE_HOME_AUTOMATION,
            DeviceId::MAINS_POWER_OUTLET,
            |endpoint| profile.configure_endpoint(endpoint),
        )
        .build_router()
}

fn end_device(profile: &mut TestProfile) -> ZigbeeDevice<MockMac, EndDevice> {
    let mut mac = MockMac::new(LOCAL_IEEE);
    mac.set_rx_delay_us(u32::MAX);
    ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .power_mode(PowerMode::AlwaysOn)
        .endpoint(
            profile.endpoint(),
            PROFILE_HOME_AUTOMATION,
            DeviceId::MAINS_POWER_OUTLET,
            |endpoint| profile.configure_endpoint(endpoint),
        )
        .build()
}

fn commissioned_state() -> PersistentSecurityState {
    let mut state = PersistentSecurityState::empty();
    state.commissioned = true;
    state.extended_pan_id = EXTENDED_PAN_ID;
    state.pan_id = PAN_ID;
    state.short_address = SHORT_ADDRESS;
    state.ieee_address = LOCAL_IEEE;
    state.channel = CHANNEL;
    state.depth = 1;
    state.parent_address = 0;
    state.update_id_valid = true;
    state.network_key = NETWORK_KEY;
    state.global_counter_limit = 0x400;
    state.tclk_present = true;
    state.trust_center_address = COORDINATOR_IEEE;
    state.trust_center_link_key = [0x5A; 16];
    state.tclk_counter_limit = 0x400;
    state
}

fn security_store() -> RamSecurityStateStore {
    let mut store = RamSecurityStateStore::new();
    store.store(&commissioned_state()).unwrap();
    store
}

#[derive(Debug, Default)]
struct LocalState {
    statuses: Vec<NetworkStatus>,
    commands: Vec<RelayCommand>,
    pending: Option<LocalRelaySelection>,
    acknowledgements: Vec<u32>,
    clear_trip: bool,
    factory_reset: bool,
    meter_timeout: Option<MeterFault>,
    armed_meter_timeout: Option<(u32, MeterFault)>,
    order: Vec<&'static str>,
}

#[derive(Clone)]
struct TestLocal(Rc<RefCell<LocalState>>);

impl LocalControl for TestLocal {
    fn set_network_status(&mut self, status: NetworkStatus) {
        self.0.borrow_mut().statuses.push(status);
    }

    fn take_local_selection(&mut self, last_sequence: &mut u32) -> Option<LocalRelaySelection> {
        let pending = self.0.borrow_mut().pending.take()?;
        if pending.sequence == *last_sequence {
            return None;
        }
        *last_sequence = pending.sequence;
        Some(pending)
    }

    fn acknowledge_local_selection(&mut self, sequence: u32) {
        let mut state = self.0.borrow_mut();
        state.acknowledgements.push(sequence);
        state.order.push("selection-ack");
    }

    fn take_clear_trip_requested(&mut self) -> bool {
        let mut state = self.0.borrow_mut();
        core::mem::take(&mut state.clear_trip)
    }

    fn take_factory_reset_requested(&mut self) -> bool {
        let mut state = self.0.borrow_mut();
        core::mem::take(&mut state.factory_reset)
    }

    fn take_meter_timeout(&mut self) -> Option<MeterFault> {
        self.0.borrow_mut().meter_timeout.take()
    }

    fn arm_meter_timeout(&mut self, timeout_ms: u32, fault: MeterFault) {
        self.0.borrow_mut().armed_meter_timeout = Some((timeout_ms, fault));
    }

    fn disarm_meter_timeout(&mut self) {
        self.0.borrow_mut().armed_meter_timeout = None;
    }

    fn release_meter_timeout_inhibit(&mut self) {
        self.0.borrow_mut().order.push("meter-inhibit-release");
    }

    fn release_factory_reset_inhibit(&mut self) {
        self.0.borrow_mut().order.push("reset-inhibit-release");
    }

    fn apply_relay(&mut self, command: RelayCommand) {
        let mut state = self.0.borrow_mut();
        state.commands.push(command);
        if !command.relay_on {
            state.order.push("relay-off");
        } else {
            state.order.push("relay-on");
        }
    }

    fn force_relay_off_sync(&mut self) {
        self.0.borrow_mut().order.push("relay-off-sync");
    }
}

#[derive(Clone)]
struct TestClock(Rc<Cell<u32>>);

impl PlugClock for TestClock {
    fn now_ms(&mut self) -> u32 {
        self.0.get()
    }
}

struct TestMeter {
    outcomes: VecDeque<MeterServiceOutcome>,
    total_energy_uwh: u64,
    restored_energy_uwh: u64,
}

impl TestMeter {
    fn new(outcomes: impl IntoIterator<Item = MeterServiceOutcome>) -> Self {
        Self {
            outcomes: outcomes.into_iter().collect(),
            total_energy_uwh: 0,
            restored_energy_uwh: 0,
        }
    }
}

impl MeterService for TestMeter {
    fn restore_energy_uwh(&mut self, total_uwh: u64) {
        self.restored_energy_uwh = total_uwh;
        self.total_energy_uwh = total_uwh;
    }

    fn total_energy_uwh(&self) -> u64 {
        self.total_energy_uwh
    }

    fn service(&mut self, _now_ms: u32) -> MeterServiceOutcome {
        let outcome = self
            .outcomes
            .pop_front()
            .unwrap_or(MeterServiceOutcome::Idle);
        if let MeterServiceOutcome::Sample(sample) = outcome {
            self.total_energy_uwh = sample.total_energy_uwh;
        }
        outcome
    }
}

struct TestNv {
    inner: RamNvStorage,
    writes: u32,
    read_error: Option<NvError>,
    write_error: Option<NvError>,
    order: Rc<RefCell<LocalState>>,
}

struct OrderedPlugSecurityStore {
    state: Option<PersistentSecurityState>,
    reset_written: bool,
    order: Rc<RefCell<LocalState>>,
}

struct RecordingChildStore {
    table: Option<PersistentChildTable>,
    loads: u32,
    stores: u32,
    order: Rc<RefCell<LocalState>>,
}

impl RecordingChildStore {
    fn empty(order: Rc<RefCell<LocalState>>) -> Self {
        Self {
            table: None,
            loads: 0,
            stores: 0,
            order,
        }
    }

    fn with_table(order: Rc<RefCell<LocalState>>, table: PersistentChildTable) -> Self {
        Self {
            table: Some(table),
            ..Self::empty(order)
        }
    }
}

impl ChildTableStore for RecordingChildStore {
    fn load(&mut self) -> Result<Option<PersistentChildTable>, ChildStoreError> {
        self.loads = self.loads.wrapping_add(1);
        self.order.borrow_mut().order.push("child-load");
        Ok(self.table.clone())
    }

    fn store(&mut self, table: &PersistentChildTable) -> Result<(), ChildStoreError> {
        table.validate()?;
        self.stores = self.stores.wrapping_add(1);
        self.order.borrow_mut().order.push(if table.is_empty() {
            "child-clear"
        } else {
            "child-save"
        });
        self.table = Some(table.clone());
        Ok(())
    }
}

impl OrderedPlugSecurityStore {
    fn new(order: Rc<RefCell<LocalState>>) -> Self {
        Self {
            state: None,
            reset_written: false,
            order,
        }
    }
}

impl SecurityStateStore for OrderedPlugSecurityStore {
    fn load(&mut self) -> Result<Option<PersistentSecurityState>, SecurityStoreError> {
        self.order.borrow_mut().order.push(if self.reset_written {
            "steering"
        } else {
            "security-load"
        });
        Ok(self.state)
    }

    fn store(&mut self, state: &PersistentSecurityState) -> Result<(), SecurityStoreError> {
        if !state.commissioned {
            self.order.borrow_mut().order.push("security-reset");
            self.reset_written = true;
        }
        self.state = Some(*state);
        Ok(())
    }
}

impl TestNv {
    fn new(order: Rc<RefCell<LocalState>>) -> Self {
        Self {
            inner: RamNvStorage::new(),
            writes: 0,
            read_error: None,
            write_error: None,
            order,
        }
    }

    fn with_read_error(order: Rc<RefCell<LocalState>>, error: NvError) -> Self {
        Self {
            read_error: Some(error),
            ..Self::new(order)
        }
    }

    fn with_write_error(order: Rc<RefCell<LocalState>>, error: NvError) -> Self {
        Self {
            write_error: Some(error),
            ..Self::new(order)
        }
    }

    fn with_record(order: Rc<RefCell<LocalState>>, record: PlugStateRecord) -> Self {
        let mut store = Self::new(order);
        store
            .inner
            .write(NvItemId::AppEndpoint1, &record.encode())
            .unwrap();
        store
    }
}

impl NvStorage for TestNv {
    fn read(&mut self, id: NvItemId, buf: &mut [u8]) -> Result<usize, NvError> {
        if let Some(error) = self.read_error {
            return Err(error);
        }
        self.inner.read(id, buf)
    }

    fn write(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError> {
        if let Some(error) = self.write_error {
            return Err(error);
        }
        self.writes = self.writes.wrapping_add(1);
        self.order.borrow_mut().order.push("checkpoint");
        self.inner.write(id, data)
    }

    fn delete(&mut self, id: NvItemId) -> Result<(), NvError> {
        self.inner.delete(id)
    }

    fn exists(&mut self, id: NvItemId) -> Result<bool, NvError> {
        self.inner.exists(id)
    }

    fn item_length(&mut self, id: NvItemId) -> Result<usize, NvError> {
        self.inner.item_length(id)
    }

    fn compact(&mut self) -> Result<(), NvError> {
        self.inner.compact()
    }
}

fn nominal_sample() -> ElectricalSample {
    ElectricalSample {
        voltage_mv: 230_000,
        current_ma: 1_000,
        active_power_mw: 200_000,
        frequency_millihz: Some(50_000),
        total_energy_uwh: 1_000,
    }
}

fn over_current_sample() -> ElectricalSample {
    ElectricalSample {
        current_ma: 20_000,
        ..nominal_sample()
    }
}

fn child_table(extended_pan_id: [u8; 8]) -> PersistentChildTable {
    let mut table = PersistentChildTable::new(extended_pan_id);
    table
        .push(PersistentChild {
            ieee_address: [0x22; 8],
            short_address: 0x2345,
            rx_on_when_idle: false,
            security_capable: true,
            is_router: false,
            end_device_timeout: 8,
        })
        .unwrap();
    table
}

fn basic_reset_frame() -> MacFrame {
    let zcl = ZclFrame::new_cluster_specific(
        0x42,
        CMD_RESET_TO_FACTORY_DEFAULTS,
        ClusterDirection::ClientToServer,
        true,
    );
    let mut zcl_bytes = [0u8; 16];
    let zcl_len = zcl.serialize(&mut zcl_bytes).unwrap();

    let aps_header = ApsHeader {
        frame_control: ApsFrameControl {
            frame_type: ApsFrameType::Data as u8,
            delivery_mode: ApsDeliveryMode::Unicast as u8,
            ack_request: true,
            ..Default::default()
        },
        dst_endpoint: Some(1),
        cluster_id: Some(zigbee_zcl::ClusterId::BASIC.0),
        profile_id: Some(PROFILE_HOME_AUTOMATION),
        src_endpoint: Some(1),
        aps_counter: 2,
        ..Default::default()
    };
    let mut aps = [0u8; 64];
    let aps_header_len = aps_header.serialize(&mut aps);
    aps[aps_header_len..aps_header_len + zcl_len].copy_from_slice(&zcl_bytes[..zcl_len]);
    let aps_len = aps_header_len + zcl_len;

    let nwk_header = NwkHeader {
        frame_control: NwkFrameControl {
            frame_type: NwkFrameType::Data as u8,
            protocol_version: 0x02,
            security: true,
            ..Default::default()
        },
        dst_addr: ShortAddress(SHORT_ADDRESS),
        src_addr: ShortAddress::COORDINATOR,
        radius: 5,
        seq_number: 2,
        dst_ieee: None,
        src_ieee: None,
        multicast_control: None,
        source_route: None,
    };
    let mut bytes = [0u8; 128];
    let nwk_header_len = nwk_header.serialize(&mut bytes);
    let security_header = NwkSecurityHeader {
        security_control: NwkSecurityHeader::ZIGBEE_DEFAULT,
        frame_counter: 2,
        source_address: COORDINATOR_IEEE,
        key_seq_number: 0,
    };
    let security_header_len = security_header.serialize(&mut bytes[nwk_header_len..]);
    let aad_len = nwk_header_len + security_header_len;
    let encrypted = NwkSecurity::new()
        .encrypt(
            &bytes[..aad_len],
            &aps[..aps_len],
            &NETWORK_KEY,
            &security_header,
        )
        .unwrap();
    bytes[aad_len..aad_len + encrypted.len()].copy_from_slice(&encrypted);
    bytes[nwk_header_len] &= !0x07;
    MacFrame::from_slice(&bytes[..aad_len + encrypted.len()]).unwrap()
}

fn mgmt_leave_frame(remove_children: bool, rejoin: bool) -> MacFrame {
    let aps_header = ApsHeader {
        frame_control: ApsFrameControl {
            frame_type: ApsFrameType::Data as u8,
            delivery_mode: ApsDeliveryMode::Unicast as u8,
            ..Default::default()
        },
        dst_endpoint: Some(0),
        cluster_id: Some(0x0034),
        profile_id: Some(0x0000),
        src_endpoint: Some(0),
        aps_counter: 3,
        ..Default::default()
    };
    let mut aps = [0u8; 64];
    let aps_header_len = aps_header.serialize(&mut aps);
    aps[aps_header_len] = 0x43;
    aps[aps_header_len + 9] = (u8::from(remove_children) << 6) | (u8::from(rejoin) << 7);
    let aps_len = aps_header_len + 10;

    let nwk_header = NwkHeader {
        frame_control: NwkFrameControl {
            frame_type: NwkFrameType::Data as u8,
            protocol_version: 0x02,
            security: true,
            ..Default::default()
        },
        dst_addr: ShortAddress(SHORT_ADDRESS),
        src_addr: ShortAddress::COORDINATOR,
        radius: 5,
        seq_number: 3,
        dst_ieee: None,
        src_ieee: None,
        multicast_control: None,
        source_route: None,
    };
    let mut bytes = [0u8; 128];
    let nwk_header_len = nwk_header.serialize(&mut bytes);
    let security_header = NwkSecurityHeader {
        security_control: NwkSecurityHeader::ZIGBEE_DEFAULT,
        frame_counter: 3,
        source_address: COORDINATOR_IEEE,
        key_seq_number: 0,
    };
    let security_header_len = security_header.serialize(&mut bytes[nwk_header_len..]);
    let aad_len = nwk_header_len + security_header_len;
    let encrypted = NwkSecurity::new()
        .encrypt(
            &bytes[..aad_len],
            &aps[..aps_len],
            &NETWORK_KEY,
            &security_header,
        )
        .unwrap();
    bytes[aad_len..aad_len + encrypted.len()].copy_from_slice(&encrypted);
    bytes[nwk_header_len] &= !0x07;
    MacFrame::from_slice(&bytes[..aad_len + encrypted.len()]).unwrap()
}

struct TestAppParts<C> {
    children: C,
    local: Rc<RefCell<LocalState>>,
    clock: Rc<Cell<u32>>,
    meter: TestMeter,
    protection: ProtectionConfig,
}

fn test_app_parts<C>(
    children: C,
    local: Rc<RefCell<LocalState>>,
    clock: Rc<Cell<u32>>,
    meter: TestMeter,
    protection: ProtectionConfig,
) -> TestAppParts<C> {
    TestAppParts {
        children,
        local,
        clock,
        meter,
        protection,
    }
}

fn app<'a, S>(
    device: &'a mut ZigbeeDevice<MockMac, Router>,
    security: &'a mut S,
    profile: &'a mut TestProfile,
    local: Rc<RefCell<LocalState>>,
    clock: Rc<Cell<u32>>,
    meter: TestMeter,
    protection: ProtectionConfig,
) -> PlugRouterApp<
    'a,
    MockMac,
    S,
    RamChildTableStore,
    TestLocal,
    NoSupervisor,
    NoDiagnostics,
    TestMeter,
    TestNv,
    TestClock,
>
where
    S: SecurityStateStore,
{
    app_with_children(
        device,
        security,
        profile,
        test_app_parts(RamChildTableStore::new(), local, clock, meter, protection),
    )
}

fn app_with_children<'a, S, C>(
    device: &'a mut ZigbeeDevice<MockMac, Router>,
    security: &'a mut S,
    profile: &'a mut TestProfile,
    parts: TestAppParts<C>,
) -> PlugRouterApp<
    'a,
    MockMac,
    S,
    C,
    TestLocal,
    NoSupervisor,
    NoDiagnostics,
    TestMeter,
    TestNv,
    TestClock,
>
where
    S: SecurityStateStore,
    C: ChildTableStore,
{
    let TestAppParts {
        children,
        local,
        clock,
        meter,
        protection,
    } = parts;
    let node = ZigbeeNode::new(device, security, profile);
    let parent = ParentRouterApp::new(
        node,
        PersistentChildren::new(children),
        &POLICY,
        RouterParts::new(
            PlugStatus::new(TestLocal(local.clone())),
            NoSupervisor,
            NoDiagnostics,
        ),
    )
    .unwrap();
    PlugRouterApp::new(
        parent,
        PlugController::new(PlugSettings::default(), protection, 30),
        meter,
        TestNv::new(local),
        TestClock(clock),
    )
    .unwrap()
}

#[test]
fn button_led_and_network_mapping_keep_stock_policy() {
    let mut gesture = ButtonGesture::new(30, 4_000);
    assert_eq!(gesture.sample(0, true), None);
    assert_eq!(gesture.sample(30, true), None);
    assert_eq!(
        gesture.sample(4_030, true),
        Some(ButtonGestureEvent::LongPress)
    );
    assert_eq!(gesture.sample(4_100, false), None);
    assert_eq!(gesture.sample(4_130, false), None);

    let archetype = NodeArchetype::ParentRouter;
    for status in [
        RouterStatus::Starting { archetype },
        RouterStatus::Commissioning {
            archetype,
            attempt: 1,
        },
        RouterStatus::Rejoining {
            archetype,
            failures: 1,
        },
        RouterStatus::Recommissioning {
            archetype,
            attempt: 2,
            retry_in_ms: 1_000,
        },
        RouterStatus::Resetting { archetype },
    ] {
        assert_eq!(network_status_for_router(status), NetworkStatus::Searching);
    }

    assert_eq!(
        network_status_for_router(RouterStatus::Online {
            archetype,
            short_address: SHORT_ADDRESS,
            identifying: false,
        }),
        NetworkStatus::Joined
    );
    assert_eq!(
        network_status_for_router(RouterStatus::Fault { archetype }),
        NetworkStatus::Fault
    );
    assert!(status_led_on(NetworkStatus::Searching, false, 0));
    assert!(!status_led_on(NetworkStatus::Searching, true, 500));
    assert!(!status_led_on(NetworkStatus::Joined, false, 0));
    assert!(status_led_on(NetworkStatus::Joined, true, 0));
}

#[test]
fn always_on_end_device_frontend_reuses_plug_behavior_without_router_claims() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(100));
    let mut profile = profile();
    let mut device = end_device(&mut profile);
    let mut security = security_store();
    let node = ZigbeeNode::new(&mut device, &mut security, &mut profile);
    let end_device = AlwaysOnEndDeviceApp::new(
        node,
        &POLICY,
        RouterParts::new(
            PlugStatus::new(TestLocal(local.clone())),
            NoSupervisor,
            NoDiagnostics,
        ),
    )
    .unwrap();
    let mut app = AlwaysOnEndDevicePlugApp::new(
        end_device,
        PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30),
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        TestNv::new(local.clone()),
        TestClock(clock),
    )
    .unwrap();

    block_on(app.initialize()).unwrap();
    assert!(app.end_device().node().device().is_joined());
    assert!(local.borrow().statuses.contains(&NetworkStatus::Joined));

    local.borrow_mut().pending = Some(LocalRelaySelection {
        sequence: 1,
        relay_on: true,
    });
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.local_selection,
        Some(LocalRelaySelection {
            sequence: 1,
            relay_on: true,
        })
    );
    assert!(app.end_device().node().profile().component().is_on());
    assert_eq!(outcome.meter, MeterServiceOutcome::Sample(nominal_sample()));
    assert_eq!(outcome.checkpoint, CheckpointOutcome::Written);
}

#[test]
fn finite_step_maps_network_status_and_keeps_one_twenty_ms_receive_slice() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::with_table(local.clone(), child_table(EXTENDED_PAN_ID));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock.clone(),
            TestMeter::new([]),
            ProtectionConfig::default(),
        ),
    );

    local.borrow_mut().order.clear();
    assert!(matches!(
        block_on(app.step()),
        Err(PlugRouterError::NotInitialized)
    ));
    block_on(app.initialize()).unwrap();
    assert!(app.router().node().device().is_joined());
    assert!(local.borrow().statuses.contains(&NetworkStatus::Searching));
    assert!(local.borrow().statuses.contains(&NetworkStatus::Joined));
    assert_eq!(app.router().children().store().loads, 1);
    assert_eq!(app.router().children().store().stores, 0);
    assert_eq!(
        local.borrow().order.first(),
        Some(&"child-load"),
        "restore completes during initialize, before the first parent-service step"
    );

    let before = app.router().node().device().mac().monotonic_micros();
    let outcome = block_on(app.step()).unwrap();
    let after = app.router().node().device().mac().monotonic_micros();
    assert!(outcome.network.is_empty());
    assert_eq!(after.wrapping_sub(before), 20_000);
    assert_eq!(outcome.checkpoint, CheckpointOutcome::Written);
    assert!(!outcome.on_off_ticked);
    assert_eq!(
        app.router().children().store().stores,
        0,
        "a restored clean child table must not be rewritten"
    );

    clock.set(99);
    assert!(!block_on(app.step()).unwrap().on_off_ticked);
    clock.set(100);
    assert!(block_on(app.step()).unwrap().on_off_ticked);
}

#[test]
fn delayed_step_consumes_all_elapsed_on_off_ticks() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local,
        clock.clone(),
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        ProtectionConfig::default(),
    );

    block_on(app.initialize()).unwrap();
    block_on(app.step()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .inner_mut()
        .on_off_mut()
        .handle_command(CMD_ON_WITH_TIMED_OFF, &[0x00, 0x03, 0x00, 0x05, 0x00])
        .unwrap();
    assert!(app.router().node().profile().component().is_on());

    clock.set(400);
    let outcome = block_on(app.step()).unwrap();
    assert!(outcome.on_off_ticked);
    assert!(!app.router().node().profile().component().is_on());
}

#[test]
fn foreign_child_record_is_cleared_during_initialize_before_parent_service() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::with_table(local.clone(), child_table([0xEE; 8]));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock,
            TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
            ProtectionConfig::default(),
        ),
    );

    local.borrow_mut().order.clear();
    block_on(app.initialize()).unwrap();
    let store = app.router().children().store();
    assert_eq!(store.loads, 1);
    assert_eq!(store.stores, 1);
    assert!(store.table.as_ref().unwrap().is_empty());
    assert_eq!(
        &local.borrow().order[..2],
        ["child-load", "child-clear"],
        "foreign state is discarded before any steady-state parent service"
    );

    block_on(app.step()).unwrap();
    assert_eq!(
        app.router().children().store().stores,
        1,
        "discarding a foreign record must leave a clean empty snapshot"
    );
}

#[test]
fn child_snapshot_writes_only_when_dirty_and_recommission_clears_it() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::empty(local.clone());
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local,
            clock,
            TestMeter::new([]),
            ProtectionConfig::default(),
        ),
    );

    block_on(app.initialize()).unwrap();
    assert_eq!(app.router().children().store().loads, 1);
    assert_eq!(app.router().children().store().stores, 0);

    block_on(app.step()).unwrap();
    assert_eq!(
        app.router().children().store().stores,
        1,
        "the initially absent dirty snapshot is committed once"
    );
    block_on(app.step()).unwrap();
    assert_eq!(
        app.router().children().store().stores,
        1,
        "an unchanged child table must not consume another journal slot"
    );

    app.router_mut()
        .node_mut()
        .device_mut()
        .user_action(UserAction::Leave);
    let outcome = block_on(app.step()).unwrap();
    assert!(matches!(outcome.network.tick, Some(StackEvent::Left)));
    assert_eq!(app.router().children().store().stores, 2);
    assert!(
        app.router()
            .children()
            .store()
            .table
            .as_ref()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pending_local_selection_is_copied_then_acknowledged() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    local.borrow_mut().pending = Some(LocalRelaySelection {
        sequence: 7,
        relay_on: true,
    });

    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.local_selection,
        Some(LocalRelaySelection {
            sequence: 7,
            relay_on: true,
        })
    );
    assert_eq!(local.borrow().acknowledgements, [7]);
    assert!(app.router().node().profile().component().is_on());
    assert!(local.borrow().commands.last().unwrap().relay_on);
}

#[test]
fn protection_vetoes_meter_trip_and_every_later_on_request() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let meter = TestMeter::new([
        MeterServiceOutcome::Sample(nominal_sample()),
        MeterServiceOutcome::Sample(over_current_sample()),
        MeterServiceOutcome::Sample(over_current_sample()),
    ]);
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        meter,
        ProtectionConfig {
            trip_delay_ms: 0,
            ..ProtectionConfig::default()
        },
    );
    block_on(app.initialize()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);

    block_on(app.step()).unwrap();
    assert!(local.borrow().commands.last().unwrap().relay_on);
    block_on(app.step()).unwrap();
    local.borrow_mut().order.clear();
    block_on(app.step()).unwrap();
    assert!(app.controller().trip_reason().is_some());
    assert!(!app.router().node().profile().component().is_on());
    assert!(local.borrow().order.contains(&"relay-off-sync"));
    assert!(!local.borrow().commands.last().unwrap().relay_on);

    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();
    assert!(!app.router().node().profile().component().is_on());
    assert!(!local.borrow().commands.last().unwrap().relay_on);
}

#[test]
fn restored_on_state_waits_for_the_first_safe_meter_sample() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let node = ZigbeeNode::new(&mut device, &mut security, &mut profile);
    let parent = ParentRouterApp::new(
        node,
        PersistentChildren::new(RamChildTableStore::new()),
        &POLICY,
        RouterParts::new(
            PlugStatus::new(TestLocal(local.clone())),
            NoSupervisor,
            NoDiagnostics,
        ),
    )
    .unwrap();
    let mut app = PlugRouterApp::new(
        parent,
        PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30),
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        TestNv::with_record(local.clone(), PlugStateRecord::new(7, true, 55)),
        TestClock(clock),
    )
    .unwrap();

    assert!(app.router().node().profile().component().is_on());
    assert!(!app.startup_outputs().relay_on);
    let startup = *local.borrow().commands.last().unwrap();
    assert!(!startup.relay_on);
    assert!(startup.relay_inhibited);

    block_on(app.initialize()).unwrap();
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(outcome.meter_safety, MeterSafetyState::Healthy);
    let healthy = *local.borrow().commands.last().unwrap();
    assert!(healthy.relay_on);
    assert!(!healthy.relay_inhibited);
}

#[test]
fn dropped_meter_input_latches_fault_and_synchronously_opens_relay() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([
            MeterServiceOutcome::Sample(nominal_sample()),
            MeterServiceOutcome::InputDropped,
        ]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();
    assert!(local.borrow().commands.last().unwrap().relay_on);

    local.borrow_mut().order.clear();
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.meter_safety,
        MeterSafetyState::FaultLatched(MeterFault::InputDropped)
    );
    assert!(!app.router().node().profile().component().is_on());
    assert!(local.borrow().order.contains(&"relay-off-sync"));
    assert!(!local.borrow().commands.last().unwrap().relay_on);
}

#[test]
fn reset_meter_interface_latches_the_same_fail_closed_path() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([
            MeterServiceOutcome::Sample(nominal_sample()),
            MeterServiceOutcome::InterfaceReset,
        ]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    local.borrow_mut().order.clear();
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.meter_safety,
        MeterSafetyState::FaultLatched(MeterFault::InterfaceReset)
    );
    assert!(local.borrow().order.contains(&"relay-off-sync"));
}

#[test]
fn stale_meter_timeout_latches_before_network_service() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock.clone(),
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    local.borrow_mut().order.clear();
    clock.set(MeterHealthPolicy::default().stale_timeout_ms);
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.meter_safety,
        MeterSafetyState::FaultLatched(MeterFault::Stale)
    );
    let relay_off = local
        .borrow()
        .order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let network_service = local
        .borrow()
        .order
        .iter()
        .position(|entry| *entry == "security-load");
    assert!(network_service.is_none() || relay_off < network_service.unwrap());
}

#[test]
fn interrupt_meter_watchdog_latches_before_the_software_deadline() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    block_on(app.step()).unwrap();
    assert_eq!(
        local.borrow().armed_meter_timeout,
        Some((
            MeterHealthPolicy::default().stale_timeout_ms,
            MeterFault::Stale
        ))
    );

    local.borrow_mut().order.clear();
    local.borrow_mut().meter_timeout = Some(MeterFault::Stale);
    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.meter_safety,
        MeterSafetyState::FaultLatched(MeterFault::Stale)
    );
    let order = &local.borrow().order;
    let relay_off = order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let release = order
        .iter()
        .position(|entry| *entry == "meter-inhibit-release")
        .unwrap();
    assert!(relay_off < release);
}

#[test]
fn missing_startup_sample_times_out_fail_closed() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(0));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock.clone(),
        TestMeter::new([]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    clock.set(MeterHealthPolicy::default().startup_timeout_ms);

    let outcome = block_on(app.step()).unwrap();
    assert_eq!(
        outcome.meter_safety,
        MeterSafetyState::FaultLatched(MeterFault::StartupTimeout)
    );
    assert!(local.borrow().order.contains(&"relay-off-sync"));
}

#[test]
fn factory_reset_orders_relay_off_before_app_checkpoint_before_network_reset() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(1_000));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::with_table(local.clone(), child_table(EXTENDED_PAN_ID));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock,
            TestMeter::new([]),
            ProtectionConfig::default(),
        ),
    );
    block_on(app.initialize()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    local.borrow_mut().order.clear();
    local.borrow_mut().factory_reset = true;
    let outcome = block_on(app.step()).unwrap();
    assert!(outcome.factory_reset_requested);
    assert!(outcome.network.is_empty());
    assert!(!app.router().node().device().is_joined());
    assert!(!app.router().node().profile().component().is_on());
    assert_eq!(
        app.router().children().store().stores,
        1,
        "urgent factory reset clears the durable child journal exactly once"
    );
    assert!(
        app.router()
            .children()
            .store()
            .table
            .as_ref()
            .unwrap()
            .is_empty()
    );
    let order = &local.borrow().order;
    let relay_off = order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let checkpoint = order
        .iter()
        .position(|entry| *entry == "checkpoint")
        .unwrap();
    let child_clear = order
        .iter()
        .position(|entry| *entry == "child-clear")
        .unwrap();
    let inhibit_release = order
        .iter()
        .position(|entry| *entry == "reset-inhibit-release")
        .unwrap();
    assert!(relay_off < checkpoint);
    assert!(checkpoint < child_clear);
    assert!(child_clear < inhibit_release);
}

#[test]
fn network_requested_reset_uses_the_same_relay_checkpoint_transaction() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(1_000));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = OrderedPlugSecurityStore::new(local.clone());
    security.state = Some(commissioned_state());
    let children = RecordingChildStore::with_table(local.clone(), child_table(EXTENDED_PAN_ID));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock,
            TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
            ProtectionConfig::default(),
        ),
    );
    block_on(app.initialize()).unwrap();
    block_on(app.step()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    local.borrow_mut().order.clear();
    app.router_mut()
        .node_mut()
        .device_mut()
        .user_action(UserAction::FactoryReset);
    let outcome = block_on(app.step()).unwrap();
    assert!(matches!(outcome.network.tick, Some(StackEvent::Left)));
    assert!(!app.router().node().profile().component().is_on());
    assert!(!app.router().node().device().is_joined());

    let order = &local.borrow().order;
    let relay_off = order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let checkpoint = order
        .iter()
        .position(|entry| *entry == "checkpoint")
        .unwrap();
    let security_reset = order
        .iter()
        .position(|entry| *entry == "security-reset")
        .unwrap();
    let child_clear = order
        .iter()
        .position(|entry| *entry == "child-clear")
        .unwrap();
    let inhibit_release = order
        .iter()
        .position(|entry| *entry == "reset-inhibit-release")
        .unwrap();
    assert!(relay_off < checkpoint);
    assert!(checkpoint < security_reset);
    assert!(security_reset < child_clear);
    assert!(child_clear < inhibit_release);
}

#[test]
fn basic_reset_synchronously_opens_and_checkpoints_without_leaving_network() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(1_000));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::with_table(local.clone(), child_table(EXTENDED_PAN_ID));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock,
            TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
            ProtectionConfig::default(),
        ),
    );
    block_on(app.initialize()).unwrap();
    block_on(app.step()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    let mac = app.router_mut().node_mut().device_mut().mac_mut();
    mac.set_rx_delay_us(0);
    mac.enqueue_rx(McpsDataIndication {
        src_address: MacAddress::Short(PanId(PAN_ID), ShortAddress::COORDINATOR),
        dst_address: MacAddress::Short(PanId(PAN_ID), ShortAddress(SHORT_ADDRESS)),
        lqi: 220,
        payload: basic_reset_frame(),
        security_use: false,
    });
    local.borrow_mut().order.clear();

    let outcome = block_on(app.step()).unwrap();
    assert!(matches!(
        outcome.network.incoming,
        Some(StackEvent::BasicResetToFactoryDefaults)
    ));
    assert!(app.router().node().device().is_joined());
    assert!(!app.router().node().profile().component().is_on());
    assert_eq!(app.router().children().store().stores, 0);

    let order = &local.borrow().order;
    let relay_off = order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let checkpoint = order
        .iter()
        .position(|entry| *entry == "checkpoint")
        .unwrap();
    assert!(relay_off < checkpoint);
    assert!(!order.contains(&"security-reset"));
    assert!(!order.contains(&"child-clear"));
}

#[test]
fn accepted_leave_does_not_clear_children_before_the_app_checkpoint() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(1_000));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = security_store();
    let children = RecordingChildStore::with_table(local.clone(), child_table(EXTENDED_PAN_ID));
    let mut app = app_with_children(
        &mut device,
        &mut security,
        &mut profile,
        test_app_parts(
            children,
            local.clone(),
            clock,
            TestMeter::new([MeterServiceOutcome::Sample(nominal_sample())]),
            ProtectionConfig::default(),
        ),
    );
    block_on(app.initialize()).unwrap();
    block_on(app.step()).unwrap();
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    block_on(app.step()).unwrap();

    let mac = app.router_mut().node_mut().device_mut().mac_mut();
    mac.set_rx_delay_us(0);
    mac.enqueue_rx(McpsDataIndication {
        src_address: MacAddress::Short(PanId(PAN_ID), ShortAddress::COORDINATOR),
        dst_address: MacAddress::Short(PanId(PAN_ID), ShortAddress(SHORT_ADDRESS)),
        lqi: 220,
        payload: mgmt_leave_frame(true, false),
        security_use: false,
    });
    app.app_state_mut().write_error = Some(NvError::HardwareError);
    local.borrow_mut().order.clear();

    assert!(matches!(
        block_on(app.step()),
        Err(PlugRouterError::AppState(NvError::HardwareError))
    ));
    assert!(app.router().factory_reset_pending());
    assert_eq!(app.router().children().store().stores, 0);
    assert_eq!(
        app.router()
            .children()
            .store()
            .table
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert!(!local.borrow().order.contains(&"child-clear"));
    assert!(!local.borrow().order.contains(&"security-reset"));

    app.app_state_mut().write_error = None;
    local.borrow_mut().order.clear();
    block_on(app.step()).unwrap();
    let order = &local.borrow().order;
    let checkpoint = order
        .iter()
        .position(|entry| *entry == "checkpoint")
        .unwrap();
    let child_clear = order
        .iter()
        .position(|entry| *entry == "child-clear")
        .unwrap();
    assert!(checkpoint < child_clear);
}

#[test]
fn local_four_second_reset_is_durable_before_an_already_due_steering_retry() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(4_030));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = OrderedPlugSecurityStore::new(local.clone());
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    assert!(!app.router().node().device().is_joined());

    block_on(
        app.router_mut()
            .node_mut()
            .device_mut()
            .mac_mut()
            .delay_micros(POLICY.join_retry_initial_ms * 1_000),
    );
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);

    let mut gesture = ButtonGesture::new(30, 4_000);
    assert_eq!(gesture.sample(0, true), None);
    assert_eq!(gesture.sample(30, true), None);
    assert_eq!(
        gesture.sample(4_030, true),
        Some(ButtonGestureEvent::LongPress)
    );

    local.borrow_mut().order.clear();
    local.borrow_mut().factory_reset = true;
    let outcome = block_on(app.step()).unwrap();
    assert!(outcome.factory_reset_requested);
    assert!(outcome.network.is_empty());
    assert!(!app.router().node().device().is_joined());
    assert!(!app.router().node().profile().component().is_on());

    let order = local.borrow();
    let relay_off = order
        .order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    let checkpoint = order
        .order
        .iter()
        .position(|entry| *entry == "checkpoint")
        .unwrap();
    let security_load = order
        .order
        .iter()
        .position(|entry| *entry == "security-load")
        .unwrap();
    let security_reset = order
        .order
        .iter()
        .position(|entry| *entry == "security-reset")
        .unwrap();
    assert!(relay_off < checkpoint);
    assert!(checkpoint < security_load);
    assert!(security_load < security_reset);
    assert!(!order.order.contains(&"steering"));
    drop(order);

    block_on(app.step()).unwrap();
    assert!(local.borrow().order.contains(&"steering"));

    let mut encoded = [0u8; PlugStateRecord::LEN];
    assert_eq!(
        app.app_state_mut()
            .read(NvItemId::AppEndpoint1, &mut encoded)
            .unwrap(),
        PlugStateRecord::LEN
    );
    assert!(!PlugStateRecord::decode(encoded).unwrap().relay_on);
}

#[test]
fn reset_checkpoint_failure_stops_before_network_reset_or_due_steering() {
    let local = Rc::new(RefCell::new(LocalState::default()));
    let clock = Rc::new(Cell::new(4_030));
    let mut profile = profile();
    let mut device = device(&mut profile);
    let mut security = OrderedPlugSecurityStore::new(local.clone());
    let mut app = app(
        &mut device,
        &mut security,
        &mut profile,
        local.clone(),
        clock,
        TestMeter::new([]),
        ProtectionConfig::default(),
    );
    block_on(app.initialize()).unwrap();
    block_on(
        app.router_mut()
            .node_mut()
            .device_mut()
            .mac_mut()
            .delay_micros(POLICY.join_retry_initial_ms * 1_000),
    );
    app.router_mut()
        .node_mut()
        .profile_mut()
        .component_mut()
        .local_set_on(true);
    app.app_state_mut().write_error = Some(NvError::HardwareError);

    local.borrow_mut().order.clear();
    local.borrow_mut().factory_reset = true;
    assert!(matches!(
        block_on(app.step()),
        Err(PlugRouterError::AppState(NvError::HardwareError))
    ));

    let order = local.borrow();
    let relay_off = order
        .order
        .iter()
        .position(|entry| *entry == "relay-off-sync")
        .unwrap();
    assert!(!order.order[relay_off + 1..].contains(&"checkpoint"));
    assert!(!order.order.contains(&"security-load"));
    assert!(!order.order.contains(&"security-reset"));
    assert!(!order.order.contains(&"steering"));
    assert!(!order.order.contains(&"reset-inhibit-release"));
}

#[test]
fn checkpoint_keeps_record_slot_format_wear_bound_and_fail_closed_errors() {
    let order = Rc::new(RefCell::new(LocalState::default()));
    let mut nv = TestNv::new(order.clone());
    let initial = PlugStateRecord::new(41, true, 9_876_543);
    nv.inner
        .write(NvItemId::AppEndpoint1, &initial.encode())
        .unwrap();
    let (mut checkpoint, restored) = AppStateCheckpoint::restore(&mut nv).unwrap();
    assert!(restored.relay_on);
    assert_eq!(restored.energy_uwh, 9_876_543);
    assert_eq!(checkpoint.sequence(), 41);

    assert_eq!(
        checkpoint
            .maybe_write(&mut nv, 10, true, 9_876_544)
            .unwrap(),
        CheckpointOutcome::Written
    );
    assert_eq!(
        checkpoint
            .maybe_write(&mut nv, 11, true, 9_876_545)
            .unwrap(),
        CheckpointOutcome::Unchanged
    );
    assert_eq!(
        checkpoint
            .maybe_write(
                &mut nv,
                10 + APP_STATE_CHECKPOINT_INTERVAL_MS,
                true,
                9_876_546,
            )
            .unwrap(),
        CheckpointOutcome::Written
    );
    assert_eq!(
        checkpoint
            .maybe_write(&mut nv, 12, false, 9_876_546)
            .unwrap(),
        CheckpointOutcome::Written
    );

    let mut encoded = [0u8; PlugStateRecord::LEN];
    assert_eq!(
        nv.read(NvItemId::AppEndpoint1, &mut encoded).unwrap(),
        PlugStateRecord::LEN
    );
    let record = PlugStateRecord::decode(encoded).unwrap();
    assert!(!record.relay_on);
    assert_eq!(record.energy_uwh, 9_876_546);

    let mut failed = TestNv::with_read_error(order.clone(), NvError::HardwareError);
    assert!(matches!(
        AppStateCheckpoint::restore(&mut failed),
        Err(NvError::HardwareError)
    ));

    let mut failed = TestNv::with_write_error(order, NvError::HardwareError);
    let (mut checkpoint, _) = AppStateCheckpoint::restore(&mut failed).unwrap();
    assert!(matches!(
        checkpoint.maybe_write(&mut failed, 0, true, 1),
        Err(NvError::HardwareError)
    ));
}
