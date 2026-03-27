#[cfg(windows)]
pub mod windows {
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
            ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const SERVICE_NAME: &str = "azbridge";
    const SERVICE_DISPLAY_NAME: &str = "Azure Relay Bridge";
    const SERVICE_DESCRIPTION: &str =
        "Creates TCP/UDP/HTTP tunnels via Azure Relay Hybrid Connections";

    /// Install azbridge as a Windows Service.
    pub fn install_service() -> anyhow::Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;

        let exe_path = std::env::current_exe()?;
        let service_info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe_path,
            launch_arguments: vec![OsString::from("--svc")],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };

        let service = manager.create_service(&service_info, ServiceAccess::CHANGE_CONFIG)?;
        service.set_description(SERVICE_DESCRIPTION)?;

        println!("Service '{SERVICE_NAME}' installed successfully.");
        Ok(())
    }

    /// Uninstall the azbridge Windows Service.
    pub fn uninstall_service() -> anyhow::Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(
            SERVICE_NAME,
            ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS | ServiceAccess::STOP,
        )?;

        // Stop if running
        if let Ok(status) = service.query_status()
            && status.current_state != ServiceState::Stopped
        {
            let _ = service.stop();
            std::thread::sleep(Duration::from_secs(2));
        }

        service.delete()?;
        println!("Service '{SERVICE_NAME}' uninstalled successfully.");
        Ok(())
    }

    /// Run as a Windows Service (called with --svc flag).
    pub fn run_as_service() -> anyhow::Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
        Ok(())
    }

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(arguments: Vec<OsString>) {
        if let Err(e) = run_service(arguments) {
            tracing::error!("Service error: {}", e);
        }
    }

    fn run_service(_arguments: Vec<OsString>) -> anyhow::Result<()> {
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop => {
                    let _ = shutdown_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            // In service mode, parse with no user-supplied args
            let cli = clap::Parser::parse_from(["azbridge", "--svc"]);
            match crate::config_loader::load_config(&cli) {
                Ok(config) => {
                    let host = std::sync::Arc::new(crate::host::Host::new(config));
                    if let Err(e) = host.start().await {
                        tracing::error!("Failed to start host: {}", e);
                        return;
                    }

                    // Wait for stop signal from SCM
                    let _ = shutdown_rx.recv();
                    host.stop().await;
                }
                Err(e) => {
                    tracing::error!("Failed to load config: {}", e);
                }
            }
        });

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn service_constants() {
            assert_eq!(SERVICE_NAME, "azbridge");
            assert_eq!(SERVICE_DISPLAY_NAME, "Azure Relay Bridge");
            assert!(!SERVICE_DESCRIPTION.is_empty());
        }

        #[test]
        fn public_api_exists() {
            // Verify the public API compiles and has the expected signatures.
            // Actual SCM operations require admin privileges.
            let _ = install_service as fn() -> anyhow::Result<()>;
            let _ = uninstall_service as fn() -> anyhow::Result<()>;
            let _ = run_as_service as fn() -> anyhow::Result<()>;
        }
    }
}
