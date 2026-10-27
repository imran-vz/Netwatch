//! macOS glue for keep awake: the running apps a session can follow, and
//! launching Utilify at login.

/// A running app with a Dock presence, as offered in "While App Is Running".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningApp {
    pub pid: i32,
    pub name: String,
}

/// Regular (Dock) apps other than Utilify, sorted by name.
#[cfg(target_os = "macos")]
pub fn running_apps() -> Vec<RunningApp> {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
    let me = std::process::id() as i32;
    let mut apps: Vec<RunningApp> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|a| {
            a.activationPolicy() == NSApplicationActivationPolicy::Regular
                && a.processIdentifier() != me
                && !a.isTerminated()
        })
        .filter_map(|a| {
            Some(RunningApp {
                pid: a.processIdentifier(),
                name: a.localizedName()?.to_string(),
            })
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

#[cfg(not(target_os = "macos"))]
pub fn running_apps() -> Vec<RunningApp> {
    Vec::new()
}

/// Launch at login, through `SMAppService` (macOS 13+). Only works from the
/// bundled app; `None` when it isn't available.
#[cfg(target_os = "macos")]
pub mod login {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    fn service() -> Option<objc2::rc::Retained<SMAppService>> {
        // macOS 11 and 12 don't have the class; messaging it would crash.
        objc2::runtime::AnyClass::get(c"SMAppService")?;
        // SAFETY: the class exists, and this is a plain class method.
        Some(unsafe { SMAppService::mainAppService() })
    }

    /// Whether Utilify is set to launch at login (`None`: unsupported).
    pub fn enabled() -> Option<bool> {
        let s = service()?;
        // SAFETY: a plain getter.
        let status = unsafe { s.status() };
        Some(
            status == SMAppServiceStatus::Enabled || status == SMAppServiceStatus::RequiresApproval,
        )
    }

    pub fn set(on: bool) -> anyhow::Result<()> {
        let s = service().ok_or_else(|| anyhow::anyhow!("needs macOS 13 or later"))?;
        // SAFETY: plain calls on the main app's service.
        let result = unsafe {
            if on {
                s.registerAndReturnError()
            } else {
                s.unregisterAndReturnError()
            }
        };
        result.map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
    }
}

#[cfg(not(target_os = "macos"))]
pub mod login {
    pub fn enabled() -> Option<bool> {
        None
    }

    pub fn set(_on: bool) -> anyhow::Result<()> {
        anyhow::bail!("unsupported")
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn lists_apps_but_not_itself() {
        let me = std::process::id() as i32;
        assert!(
            super::running_apps()
                .iter()
                .all(|a| a.pid != me && !a.name.is_empty())
        );
    }
}
