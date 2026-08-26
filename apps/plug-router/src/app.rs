//! Smart-plug state machine layered on a statically selected router frontend.

use core::future::Future;

#[cfg(feature = "router")]
use router_app::ParentRouterApp;
use router_app::{AlwaysOnEndDeviceApp, Diagnostics, RouterAppError, StepEvents, Supervisor};
use zigbee_mac::MacDriver;
#[cfg(feature = "router")]
use zigbee_mac::ParentMacDriver;
use zigbee_plug_controller::{NetworkStatus, PlugController, RelayLedState};
use zigbee_plug_profile::ZigbeePlug;
#[cfg(feature = "router")]
use zigbee_runtime::child_store::ChildTableStore;
use zigbee_runtime::nv_storage::{NvError, NvStorage};
use zigbee_runtime::profile::DeviceProfile;
use zigbee_runtime::security_store::SecurityStateStore;

use crate::capabilities::{LocalControl, LocalRelaySelection, PlugClock, RelayCommand};
use crate::meter::{
    MeterFault, MeterHealthPolicy, MeterSafetyState, MeterService, MeterServiceOutcome,
};
use crate::persistence::{AppStateCheckpoint, CheckpointOutcome};
use crate::status::PlugStatus;

const ON_OFF_TICK_MS: u32 = 100;

type PlugProfile = DeviceProfile<ZigbeePlug>;
#[cfg(feature = "router")]
type PlugParent<'a, M, S, C, L, Sv, D> =
    ParentRouterApp<'a, M, S, PlugProfile, C, PlugStatus<L>, Sv, D>;
type PlugEndDevice<'a, M, S, L, Sv, D> =
    AlwaysOnEndDeviceApp<'a, M, S, PlugProfile, PlugStatus<L>, Sv, D>;
#[cfg(feature = "router")]
type ParentPlugCore<'a, M, S, C, L, Sv, D, Me, N, Cl> =
    PlugCore<PlugParent<'a, M, S, C, L, Sv, D>, Me, N, Cl>;
type EndDevicePlugCore<'a, M, S, L, Sv, D, Me, N, Cl> =
    PlugCore<PlugEndDevice<'a, M, S, L, Sv, D>, Me, N, Cl>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlugRouterError {
    AlreadyInitialized,
    NotInitialized,
    InvalidMeterHealthPolicy,
    Router(RouterAppError),
    AppState(NvError),
}

impl From<RouterAppError> for PlugRouterError {
    fn from(error: RouterAppError) -> Self {
        Self::Router(error)
    }
}

impl From<NvError> for PlugRouterError {
    fn from(error: NvError) -> Self {
        Self::AppState(error)
    }
}

#[derive(Debug)]
pub struct PlugStepOutcome {
    pub network: StepEvents,
    pub meter: MeterServiceOutcome,
    pub meter_safety: MeterSafetyState,
    pub checkpoint: CheckpointOutcome,
    pub local_selection: Option<LocalRelaySelection>,
    pub factory_reset_requested: bool,
    pub on_off_ticked: bool,
}

struct MeterSafety {
    policy: MeterHealthPolicy,
    monitoring_since_ms: u32,
    last_sample_ms: Option<u32>,
    ready: bool,
    fault: Option<MeterFault>,
    started: bool,
}

impl MeterSafety {
    const fn new(policy: MeterHealthPolicy) -> Self {
        Self {
            policy,
            monitoring_since_ms: 0,
            last_sample_ms: None,
            ready: false,
            fault: None,
            started: false,
        }
    }

    fn begin(&mut self, now_ms: u32) {
        self.monitoring_since_ms = now_ms;
        self.last_sample_ms = None;
        self.ready = false;
        self.started = true;
    }

    fn observe_sample(&mut self, now_ms: u32, safe: bool) {
        self.last_sample_ms = Some(now_ms);
        if self.fault.is_none() && !self.ready {
            self.ready = safe;
        }
    }

    fn timeout_fault(&self, now_ms: u32) -> Option<MeterFault> {
        if !self.started || self.fault.is_some() {
            return None;
        }
        match self.last_sample_ms {
            Some(last_sample_ms)
                if now_ms.wrapping_sub(last_sample_ms) >= self.policy.stale_timeout_ms =>
            {
                Some(MeterFault::Stale)
            }
            None if now_ms.wrapping_sub(self.monitoring_since_ms)
                >= self.policy.startup_timeout_ms =>
            {
                Some(MeterFault::StartupTimeout)
            }
            _ => None,
        }
    }

    fn latch(&mut self, fault: MeterFault) {
        self.ready = false;
        if self.fault.is_none() {
            self.fault = Some(fault);
        }
    }

    fn clear_latch(&mut self, now_ms: u32) {
        self.fault = None;
        self.begin(now_ms);
    }

    const fn relay_allowed(&self) -> bool {
        self.ready && self.fault.is_none()
    }

    const fn state(&self) -> MeterSafetyState {
        match self.fault {
            Some(fault) => MeterSafetyState::FaultLatched(fault),
            None if self.ready => MeterSafetyState::Healthy,
            None => MeterSafetyState::AwaitingFirstSafeSample,
        }
    }
}

/// The exact network surface needed by the private plug behavior core.
///
/// This is deliberately private and narrow. It is not a platform abstraction:
/// the only implementations are the two statically typed `router-app`
/// frontends used by this crate.
trait PlugNetworkFrontend {
    type Local: LocalControl;

    fn plug_mut(&mut self) -> &mut ZigbeePlug;
    fn local_mut(&mut self) -> &mut Self::Local;
    fn factory_reset_pending(&self) -> bool;
    fn initialize_frontend(&mut self) -> impl Future<Output = Result<(), RouterAppError>> + '_;
    fn step_frontend(&mut self) -> impl Future<Output = Result<StepEvents, RouterAppError>> + '_;
    async fn urgent_factory_reset_frontend(&mut self) -> Result<(), RouterAppError>;
}

#[cfg(feature = "router")]
impl<'a, M, S, C, L, Sv, D> PlugNetworkFrontend for PlugParent<'a, M, S, C, L, Sv, D>
where
    M: ParentMacDriver,
    S: SecurityStateStore,
    C: ChildTableStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
{
    type Local = L;

    fn plug_mut(&mut self) -> &mut ZigbeePlug {
        self.node_mut().profile_mut().component_mut()
    }

    fn local_mut(&mut self) -> &mut Self::Local {
        self.parts_mut().status.local_mut()
    }

    fn factory_reset_pending(&self) -> bool {
        ParentRouterApp::factory_reset_pending(self)
    }

    fn initialize_frontend(&mut self) -> impl Future<Output = Result<(), RouterAppError>> + '_ {
        self.initialize_deferred_factory_reset()
    }

    fn step_frontend(&mut self) -> impl Future<Output = Result<StepEvents, RouterAppError>> + '_ {
        self.step_deferred_factory_reset()
    }

    async fn urgent_factory_reset_frontend(&mut self) -> Result<(), RouterAppError> {
        if self.factory_reset_pending() {
            self.complete_pending_factory_reset_and_recommission().await
        } else {
            self.urgent_factory_reset_and_recommission().await
        }
    }
}

impl<'a, M, S, L, Sv, D> PlugNetworkFrontend for PlugEndDevice<'a, M, S, L, Sv, D>
where
    M: MacDriver,
    S: SecurityStateStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
{
    type Local = L;

    fn plug_mut(&mut self) -> &mut ZigbeePlug {
        self.node_mut().profile_mut().component_mut()
    }

    fn local_mut(&mut self) -> &mut Self::Local {
        self.parts_mut().status.local_mut()
    }

    fn factory_reset_pending(&self) -> bool {
        AlwaysOnEndDeviceApp::factory_reset_pending(self)
    }

    fn initialize_frontend(&mut self) -> impl Future<Output = Result<(), RouterAppError>> + '_ {
        self.initialize_deferred_factory_reset()
    }

    fn step_frontend(&mut self) -> impl Future<Output = Result<StepEvents, RouterAppError>> + '_ {
        self.step_deferred_factory_reset()
    }

    async fn urgent_factory_reset_frontend(&mut self) -> Result<(), RouterAppError> {
        if self.factory_reset_pending() {
            self.complete_pending_factory_reset_and_recommission().await
        } else {
            self.urgent_factory_reset_and_recommission().await
        }
    }
}

/// The single implementation of plug behavior shared by parent-router and
/// always-on End Device frontends.
struct PlugCore<R, Me, N, Cl>
where
    R: PlugNetworkFrontend,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    router: R,
    controller: PlugController,
    meter: Me,
    app_state: N,
    checkpoint: AppStateCheckpoint,
    clock: Cl,
    hundred_ms_anchor: u32,
    local_relay_sequence: u32,
    startup_outputs: RelayLedState,
    meter_safety: MeterSafety,
    initialized: bool,
}

impl<R, Me, N, Cl> PlugCore<R, Me, N, Cl>
where
    R: PlugNetworkFrontend,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    fn new(
        mut router: R,
        mut controller: PlugController,
        mut meter: Me,
        mut app_state: N,
        mut clock: Cl,
        meter_health: MeterHealthPolicy,
    ) -> Result<Self, PlugRouterError> {
        if !meter_health.is_valid() {
            return Err(PlugRouterError::InvalidMeterHealthPolicy);
        }
        let (checkpoint, restored) = AppStateCheckpoint::restore(&mut app_state)?;
        meter.restore_energy_uwh(restored.energy_uwh);
        let _desired_startup = controller.apply_startup(router.plug_mut(), restored.relay_on);
        let startup_outputs = RelayLedState {
            relay_on: false,
            led_on: false,
        };
        let command = RelayCommand {
            relay_on: false,
            safety_tripped: controller.trip_reason().is_some(),
            relay_inhibited: true,
        };
        router.local_mut().apply_relay(command);
        let now_ms = clock.now_ms();

        Ok(Self {
            router,
            controller,
            meter,
            app_state,
            checkpoint,
            clock,
            hundred_ms_anchor: now_ms,
            local_relay_sequence: 0,
            startup_outputs,
            meter_safety: MeterSafety::new(meter_health),
            initialized: false,
        })
    }

    const fn startup_outputs(&self) -> RelayLedState {
        self.startup_outputs
    }

    const fn controller(&self) -> &PlugController {
        &self.controller
    }

    fn meter(&self) -> &Me {
        &self.meter
    }

    fn meter_mut(&mut self) -> &mut Me {
        &mut self.meter
    }

    fn app_state(&self) -> &N {
        &self.app_state
    }

    fn app_state_mut(&mut self) -> &mut N {
        &mut self.app_state
    }

    fn clock_mut(&mut self) -> &mut Cl {
        &mut self.clock
    }

    const fn meter_safety_state(&self) -> MeterSafetyState {
        self.meter_safety.state()
    }

    fn apply_outputs(&mut self, state: RelayLedState) {
        let safety_tripped =
            self.controller.trip_reason().is_some() || self.meter_safety.fault.is_some();
        let command = RelayCommand {
            relay_on: state.relay_on && self.meter_safety.relay_allowed() && !safety_tripped,
            safety_tripped,
            relay_inhibited: !self.meter_safety.relay_allowed() || safety_tripped,
        };
        let local = self.router.local_mut();
        local.apply_relay(command);
        if safety_tripped {
            local.force_relay_off_sync();
            local.set_network_status(NetworkStatus::Fault);
        }
    }

    fn reconcile(&mut self) -> RelayLedState {
        if self.meter_safety.fault.is_some() && self.router.plug_mut().is_on() {
            self.router.plug_mut().local_set_on(false);
        }
        let state = self.controller.reconcile(self.router.plug_mut());
        self.apply_outputs(state);
        state
    }

    fn sync_local_control(&mut self, now_ms: u32) -> Option<LocalRelaySelection> {
        let selection = self
            .router
            .local_mut()
            .take_local_selection(&mut self.local_relay_sequence);
        if let Some(selection) = selection {
            self.router.plug_mut().local_set_on(selection.relay_on);
            self.router
                .local_mut()
                .acknowledge_local_selection(selection.sequence);
        }
        if self.router.local_mut().take_clear_trip_requested() {
            self.controller.clear_protection_latch();
            self.meter_safety.clear_latch(now_ms);
            self.router.local_mut().arm_meter_timeout(
                self.meter_safety.policy.startup_timeout_ms,
                MeterFault::StartupTimeout,
            );
        }
        self.reconcile();
        selection
    }

    fn take_factory_reset_requested(&mut self) -> bool {
        self.router.local_mut().take_factory_reset_requested()
    }

    async fn urgent_factory_reset(
        &mut self,
        now_ms: u32,
    ) -> Result<CheckpointOutcome, PlugRouterError> {
        self.router.plug_mut().local_set_on(false);
        self.reconcile();
        let local = self.router.local_mut();
        local.apply_relay(RelayCommand {
            relay_on: false,
            safety_tripped: true,
            relay_inhibited: true,
        });
        local.force_relay_off_sync();
        local.set_network_status(NetworkStatus::Searching);
        let checkpoint = self.checkpoint.write_relay_off(
            &mut self.app_state,
            now_ms,
            self.meter.total_energy_uwh(),
        )?;
        self.router.urgent_factory_reset_frontend().await?;
        self.router.local_mut().release_factory_reset_inhibit();
        Ok(checkpoint)
    }

    fn latch_meter_fault(&mut self, fault: MeterFault) {
        self.meter_safety.latch(fault);
        self.router.local_mut().disarm_meter_timeout();
        self.router.plug_mut().local_set_on(false);
        self.reconcile();
    }

    fn enforce_meter_timeout(&mut self, now_ms: u32) {
        if let Some(fault) = self.router.local_mut().take_meter_timeout() {
            self.latch_meter_fault(fault);
            self.router.local_mut().release_meter_timeout_inhibit();
            return;
        }
        if let Some(fault) = self.meter_safety.timeout_fault(now_ms) {
            self.latch_meter_fault(fault);
        }
    }

    fn process_meter(&mut self, now_ms: u32, outcome: MeterServiceOutcome) {
        match outcome {
            MeterServiceOutcome::Sample(sample) => {
                let sample_safe = self.controller.sample_is_safe(sample);
                let _ = self
                    .controller
                    .on_sample(self.router.plug_mut(), now_ms, sample);
                self.meter_safety.observe_sample(now_ms, sample_safe);
                if self.meter_safety.fault.is_none() {
                    self.router.local_mut().arm_meter_timeout(
                        self.meter_safety.policy.stale_timeout_ms,
                        MeterFault::Stale,
                    );
                }
                self.reconcile();
            }
            MeterServiceOutcome::InputDropped => {
                self.latch_meter_fault(MeterFault::InputDropped);
            }
            MeterServiceOutcome::InterfaceReset => {
                self.latch_meter_fault(MeterFault::InterfaceReset);
            }
            MeterServiceOutcome::Idle => {}
        }
    }

    async fn initialize(&mut self) -> Result<(), PlugRouterError> {
        if self.initialized {
            return Err(PlugRouterError::AlreadyInitialized);
        }
        self.initialized = true;
        self.router.initialize_frontend().await?;
        let now_ms = self.clock.now_ms();
        self.meter_safety.begin(now_ms);
        self.router.local_mut().arm_meter_timeout(
            self.meter_safety.policy.startup_timeout_ms,
            MeterFault::StartupTimeout,
        );
        self.sync_local_control(now_ms);
        let factory_reset_requested = self.take_factory_reset_requested();
        if self.router.factory_reset_pending() || factory_reset_requested {
            let _ = self.urgent_factory_reset(now_ms).await?;
        }
        Ok(())
    }

    async fn step(&mut self) -> Result<PlugStepOutcome, PlugRouterError> {
        if !self.initialized {
            return Err(PlugRouterError::NotInitialized);
        }

        let step_started_ms = self.clock.now_ms();
        self.enforce_meter_timeout(step_started_ms);
        let local_selection = self.sync_local_control(step_started_ms);
        let mut factory_reset_requested = self.take_factory_reset_requested();
        if self.router.factory_reset_pending() || factory_reset_requested {
            let checkpoint = self.urgent_factory_reset(step_started_ms).await?;
            return Ok(PlugStepOutcome {
                network: StepEvents::default(),
                meter: MeterServiceOutcome::Idle,
                meter_safety: self.meter_safety.state(),
                checkpoint,
                local_selection,
                factory_reset_requested,
                on_off_ticked: false,
            });
        }

        let meter_now_ms = self.clock.now_ms();
        let meter = self.meter.service(meter_now_ms);
        self.process_meter(meter_now_ms, meter);

        let network = self.router.step_frontend().await?;

        // Recheck age after a potentially long platform operation, then
        // reconcile remote commands only while metering is still healthy.
        let now_ms = self.clock.now_ms();
        self.enforce_meter_timeout(now_ms);
        factory_reset_requested |= self.take_factory_reset_requested();
        if self.router.factory_reset_pending() || factory_reset_requested {
            let checkpoint = self.urgent_factory_reset(now_ms).await?;
            return Ok(PlugStepOutcome {
                network,
                meter,
                meter_safety: self.meter_safety.state(),
                checkpoint,
                local_selection,
                factory_reset_requested,
                on_off_ticked: false,
            });
        }

        let basic_reset = network.iter().any(|event| {
            matches!(
                event,
                zigbee_runtime::event_loop::StackEvent::BasicResetToFactoryDefaults
            )
        });
        let basic_reset_checkpoint = if basic_reset {
            self.router.plug_mut().local_set_on(false);
            self.reconcile();
            self.router.local_mut().force_relay_off_sync();
            Some(self.checkpoint.write_relay_off(
                &mut self.app_state,
                now_ms,
                self.meter.total_energy_uwh(),
            )?)
        } else {
            None
        };
        self.reconcile();

        let mut on_off_ticked = false;
        let elapsed_ms = now_ms.wrapping_sub(self.hundred_ms_anchor);
        let elapsed_deciseconds = elapsed_ms / ON_OFF_TICK_MS;
        if elapsed_deciseconds != 0 {
            self.hundred_ms_anchor = self
                .hundred_ms_anchor
                .wrapping_add(elapsed_deciseconds * ON_OFF_TICK_MS);
            let state = self
                .controller
                .tick_100ms_by(self.router.plug_mut(), elapsed_deciseconds);
            self.apply_outputs(state);
            on_off_ticked = true;
        }

        let relay_on = self.router.plug_mut().is_on();
        let checkpoint = if let Some(checkpoint) = basic_reset_checkpoint {
            checkpoint
        } else {
            self.checkpoint.maybe_write(
                &mut self.app_state,
                now_ms,
                relay_on,
                self.meter.total_energy_uwh(),
            )?
        };

        Ok(PlugStepOutcome {
            network,
            meter,
            meter_safety: self.meter_safety.state(),
            checkpoint,
            local_selection,
            factory_reset_requested,
            on_off_ticked,
        })
    }
}

/// Child-capable smart-plug frontend for MACs with audited parent support.
///
/// This retains the original public API used by all six TLSR8258 products.
#[cfg(feature = "router")]
pub struct PlugRouterApp<'a, M, S, C, L, Sv, D, Me, N, Cl>
where
    M: ParentMacDriver,
    S: SecurityStateStore,
    C: ChildTableStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    core: ParentPlugCore<'a, M, S, C, L, Sv, D, Me, N, Cl>,
}

#[cfg(feature = "router")]
impl<'a, M, S, C, L, Sv, D, Me, N, Cl> PlugRouterApp<'a, M, S, C, L, Sv, D, Me, N, Cl>
where
    M: ParentMacDriver,
    S: SecurityStateStore,
    C: ChildTableStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    pub fn new(
        router: PlugParent<'a, M, S, C, L, Sv, D>,
        controller: PlugController,
        meter: Me,
        app_state: N,
        clock: Cl,
    ) -> Result<Self, PlugRouterError> {
        Self::new_with_meter_health(
            router,
            controller,
            meter,
            app_state,
            clock,
            MeterHealthPolicy::default(),
        )
    }

    pub fn new_with_meter_health(
        router: PlugParent<'a, M, S, C, L, Sv, D>,
        controller: PlugController,
        meter: Me,
        app_state: N,
        clock: Cl,
        meter_health: MeterHealthPolicy,
    ) -> Result<Self, PlugRouterError> {
        Ok(Self {
            core: PlugCore::new(router, controller, meter, app_state, clock, meter_health)?,
        })
    }

    pub const fn startup_outputs(&self) -> RelayLedState {
        self.core.startup_outputs()
    }

    pub const fn router(&self) -> &PlugParent<'a, M, S, C, L, Sv, D> {
        &self.core.router
    }

    pub fn router_mut(&mut self) -> &mut PlugParent<'a, M, S, C, L, Sv, D> {
        &mut self.core.router
    }

    pub const fn controller(&self) -> &PlugController {
        self.core.controller()
    }

    pub fn meter(&self) -> &Me {
        self.core.meter()
    }

    pub fn meter_mut(&mut self) -> &mut Me {
        self.core.meter_mut()
    }

    pub fn app_state(&self) -> &N {
        self.core.app_state()
    }

    pub fn app_state_mut(&mut self) -> &mut N {
        self.core.app_state_mut()
    }

    pub fn clock_mut(&mut self) -> &mut Cl {
        self.core.clock_mut()
    }

    pub const fn meter_safety_state(&self) -> MeterSafetyState {
        self.core.meter_safety_state()
    }

    pub async fn initialize(&mut self) -> Result<(), PlugRouterError> {
        self.core.initialize().await
    }

    pub async fn step(&mut self) -> Result<PlugStepOutcome, PlugRouterError> {
        self.core.step().await
    }
}

/// Conformant always-on End Device smart-plug frontend.
///
/// It owns the exact same private plug behavior core as the parent-router
/// frontend,
/// but does not advertise routing or child capability.
pub struct AlwaysOnEndDevicePlugApp<'a, M, S, L, Sv, D, Me, N, Cl>
where
    M: MacDriver,
    S: SecurityStateStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    core: EndDevicePlugCore<'a, M, S, L, Sv, D, Me, N, Cl>,
}

impl<'a, M, S, L, Sv, D, Me, N, Cl> AlwaysOnEndDevicePlugApp<'a, M, S, L, Sv, D, Me, N, Cl>
where
    M: MacDriver,
    S: SecurityStateStore,
    L: LocalControl,
    Sv: Supervisor,
    D: Diagnostics,
    Me: MeterService,
    N: NvStorage,
    Cl: PlugClock,
{
    pub fn new(
        end_device: PlugEndDevice<'a, M, S, L, Sv, D>,
        controller: PlugController,
        meter: Me,
        app_state: N,
        clock: Cl,
    ) -> Result<Self, PlugRouterError> {
        Self::new_with_meter_health(
            end_device,
            controller,
            meter,
            app_state,
            clock,
            MeterHealthPolicy::default(),
        )
    }

    pub fn new_with_meter_health(
        end_device: PlugEndDevice<'a, M, S, L, Sv, D>,
        controller: PlugController,
        meter: Me,
        app_state: N,
        clock: Cl,
        meter_health: MeterHealthPolicy,
    ) -> Result<Self, PlugRouterError> {
        Ok(Self {
            core: PlugCore::new(
                end_device,
                controller,
                meter,
                app_state,
                clock,
                meter_health,
            )?,
        })
    }

    pub const fn startup_outputs(&self) -> RelayLedState {
        self.core.startup_outputs()
    }

    pub const fn end_device(&self) -> &PlugEndDevice<'a, M, S, L, Sv, D> {
        &self.core.router
    }

    pub fn end_device_mut(&mut self) -> &mut PlugEndDevice<'a, M, S, L, Sv, D> {
        &mut self.core.router
    }

    pub const fn controller(&self) -> &PlugController {
        self.core.controller()
    }

    pub fn meter(&self) -> &Me {
        self.core.meter()
    }

    pub fn meter_mut(&mut self) -> &mut Me {
        self.core.meter_mut()
    }

    pub fn app_state(&self) -> &N {
        self.core.app_state()
    }

    pub fn app_state_mut(&mut self) -> &mut N {
        self.core.app_state_mut()
    }

    pub fn clock_mut(&mut self) -> &mut Cl {
        self.core.clock_mut()
    }

    pub const fn meter_safety_state(&self) -> MeterSafetyState {
        self.core.meter_safety_state()
    }

    pub async fn initialize(&mut self) -> Result<(), PlugRouterError> {
        self.core.initialize().await
    }

    pub async fn step(&mut self) -> Result<PlugStepOutcome, PlugRouterError> {
        self.core.step().await
    }
}
