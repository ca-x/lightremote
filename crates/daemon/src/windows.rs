//! Native SCM integration. The windows-service crate owns all service FFI.
use crate::{config, runtime};
use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

const NAME: &str = "LightRemoteDaemon";
static CONFIG: OnceLock<PathBuf> = OnceLock::new();
static EXIT_CODE: AtomicU8 = AtomicU8::new(1);

define_windows_service!(ffi_service_main, service_main);

pub fn dispatch(path: PathBuf) -> Result<()> {
    CONFIG
        .set(path)
        .map_err(|_| anyhow::anyhow!("SCM dispatcher already started"))?;
    service_dispatcher::start(NAME, ffi_service_main)
        .context("cannot connect to Windows SCM; --service must be launched by SCM")?;
    match EXIT_CODE.load(Ordering::Acquire) {
        0 => Ok(()),
        config::RESTART_EXIT_CODE => Err(lightremote::update::RestartRequired.into()),
        _ => anyhow::bail!(
            "Windows service stopped with an error; inspect configuration and SCM service status"
        ),
    }
}

fn status(
    handle: ServiceStatusHandle,
    state: ServiceState,
    code: u8,
    checkpoint: u32,
) -> Result<()> {
    let pending = matches!(
        state,
        ServiceState::StartPending | ServiceState::StopPending
    );
    handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(u32::from(code)),
            checkpoint,
            wait_hint: if pending {
                Duration::from_secs(30)
            } else {
                Duration::ZERO
            },
            process_id: None,
        })
        .context("cannot report Windows service status")
}

fn service_main(_arguments: Vec<OsString>) {
    // This callback returns only after the engine and all its workers are joined.
    let result = serve();
    EXIT_CODE.store(config::exit_code(&result), Ordering::Release);
}

fn serve() -> Result<()> {
    let (stop, stopped) = watch::channel(false);
    let handler = move |control| match control {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop.send_replace(true);
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let handle = service_control_handler::register(NAME, handler)
        .context("cannot register SCM control handler")?;
    status(handle, ServiceState::StartPending, 0, 1)?;
    let result = (|| {
        let path = CONFIG.get().context("SCM configuration path is missing")?;
        let options = config::load(path, true)?.prepare()?;
        let runtime = runtime()?;
        status(handle, ServiceState::Running, 0, 0)?;
        runtime.block_on(lightremote::host::run_with_shutdown(options, async move {
            crate::shutdown::wait(stopped).await?;
            status(handle, ServiceState::StopPending, 0, 1)?;
            Ok(())
        }))
    })();
    // Also report StopPending for internal errors and update exits. The engine
    // has already completed its cleanup when its future returns.
    let pending = status(handle, ServiceState::StopPending, 0, 2);
    let code = config::exit_code(&result);
    let finished = status(handle, ServiceState::Stopped, code, 0);
    result?;
    pending?;
    finished
}
