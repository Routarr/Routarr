//! The Windows service. Windows starts `routarr service`, which hands its thread to the service
//! manager. The server then runs as it does from a console, stopped by the manager's controls,
//! and each state the manager waits for is reported, again and again while it is pending.

use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::{define_windows_service, service_dispatcher};

/// The name the installer registers the service under.
pub const NAME: &str = "Routarr";

/// How long the manager is told a start or a stop may take until the next report. A stop drains
/// the requests, waits up to 15 s for the moves being recorded, then checkpoints the database.
const WAIT_HINT: Duration = Duration::from_secs(30);

/// How often a pending start or stop is reported again, well inside `WAIT_HINT`.
const AGAIN: Duration = Duration::from_secs(5);

/// The state last reported, and the checkpoint a pending one has reached.
struct Reported {
    handle: ServiceStatusHandle,
    state: Mutex<(ServiceState, u32)>,
}

static REPORTED: OnceLock<Reported> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Hand this thread to the service manager, which runs `service_main` on a thread of its own.
/// From a console, the manager refuses at once.
pub fn dispatch() -> std::process::ExitCode {
    match service_dispatcher::start(NAME, ffi_service_main) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "Error: Windows runs `routarr service` as the Routarr service. Run `routarr` to \
                 start it from a console ({e})"
            );
            std::process::ExitCode::FAILURE
        }
    }
}

fn service_main(_: Vec<OsString>) {
    let (asked, stops) = tokio::sync::watch::channel(0u32);
    let controls = move |control| match control {
        ServiceControl::Stop | ServiceControl::Preshutdown => {
            asked.send_modify(|count| *count += 1);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(handle) = service_control_handler::register(NAME, controls) else { return };
    let reported = REPORTED
        .get_or_init(|| Reported { handle, state: Mutex::new((ServiceState::StopPending, 0)) });
    reported.set(ServiceState::StartPending, 0);
    std::thread::spawn(|| report_while_pending(reported));
    let status = crate::process::run_to_end(
        crate::commands::Invocation::Serve,
        crate::process::Stops::Service(stops),
    );
    reported.set(ServiceState::Stopped, status);
}

/// Tell the manager Routarr serves.
pub fn ready() {
    if let Some(reported) = REPORTED.get() {
        reported.set(ServiceState::Running, 0);
    }
}

/// Tell the manager Routarr has begun to stop.
pub fn stopping() {
    if let Some(reported) = REPORTED.get() {
        reported.set(ServiceState::StopPending, 0);
    }
}

/// A pending state reported again with its checkpoint advanced, so the manager keeps waiting
/// through a long migration or a slow stop.
fn report_while_pending(reported: &Reported) {
    loop {
        std::thread::sleep(AGAIN);
        let state = reported.state.lock().map(|state| state.0).unwrap_or(ServiceState::Stopped);
        match state {
            ServiceState::StartPending | ServiceState::StopPending => reported.set(state, 0),
            ServiceState::Stopped => return,
            _ => {}
        }
    }
}

impl Reported {
    /// Report `state`, a pending one with the next checkpoint, `Stopped` with `status`, the exit
    /// status of the process a console start would have had.
    fn set(&self, state: ServiceState, status: u8) {
        let Ok(mut held) = self.state.lock() else { return };
        let checkpoint = if held.0 == state { held.1 + 1 } else { 1 };
        *held = (state, checkpoint);
        let pending = matches!(state, ServiceState::StartPending | ServiceState::StopPending);
        let controls_accepted = match state {
            ServiceState::StartPending | ServiceState::Running => {
                ServiceControlAccept::STOP | ServiceControlAccept::PRESHUTDOWN
            }
            _ => ServiceControlAccept::empty(),
        };
        let exit_code = match status {
            0 => ServiceExitCode::Win32(0),
            status => ServiceExitCode::ServiceSpecific(u32::from(status)),
        };
        let reported = self.handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted,
            exit_code,
            checkpoint: if pending { checkpoint } else { 0 },
            wait_hint: if pending { WAIT_HINT } else { Duration::ZERO },
            process_id: None,
        });
        if let Err(e) = reported {
            tracing::warn!("The Windows service manager could not be told {state:?}: {e}");
        }
    }
}
