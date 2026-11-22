use tokio::process::Command;

/// 在 Linux 上通过 `PR_SET_PDEATHSIG` 绑定父子生死，主进程异常崩亡时内核自动向 Worker 发送 `SIGTERM`。
pub fn configure_child_deathsig(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `pre_exec` 闭包内仅调用异步信号安全的 `libc::prctl(PR_SET_PDEATHSIG, SIGTERM)`。
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cmd;
    }
}

#[cfg(windows)]
mod win_job {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::RawHandle;
    use tokio::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// Win32 Job Object 守卫：主进程句柄销毁时操作系统自动清理全部子进程，防止孤儿残留。
    pub struct ChildJobGuard {
        handle: HANDLE,
    }

    // SAFETY: Win32 Job Handle 可在同进程内的线程间安全传递与释放。
    unsafe impl Send for ChildJobGuard {}
    // SAFETY: `ChildJobGuard` 仅通过不可变句柄绑定或释放资源，且仅在内部同步点访问。
    unsafe impl Sync for ChildJobGuard {}

    impl ChildJobGuard {
        pub fn new() -> anyhow::Result<Self> {
            // SAFETY: 传入默认安全属性与匿名 Job 对象。
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(anyhow::anyhow!(
                    "CreateJobObjectW 失败: {}",
                    std::io::Error::last_os_error()
                ));
            }

            // SAFETY: 零初始化 Win32 扩展限制结构体并启用 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                // SAFETY: 设置失败时及时关闭已分配的 Job 句柄。
                unsafe {
                    CloseHandle(handle);
                }
                return Err(anyhow::anyhow!("SetInformationJobObject 失败: {err}"));
            }

            Ok(Self { handle })
        }

        pub fn empty() -> Self {
            Self {
                handle: std::ptr::null_mut(),
            }
        }

        pub fn assign_child(&self, child: &Child) -> anyhow::Result<()> {
            if self.handle.is_null() {
                return Ok(());
            }
            let Some(raw) = child.raw_handle() else {
                return Ok(());
            };
            self.assign_raw(raw)
        }

        fn assign_raw(&self, raw: RawHandle) -> anyhow::Result<()> {
            // SAFETY: `self.handle` 为有效 Job 句柄，`raw` 来自仍在生命周期内的 `Child`。
            let ok = unsafe { AssignProcessToJobObject(self.handle, raw as HANDLE) };
            if ok == 0 {
                return Err(anyhow::anyhow!(
                    "AssignProcessToJobObject 失败: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl Drop for ChildJobGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: 句柄仅在此处关闭一次。
                unsafe {
                    CloseHandle(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(not(windows))]
mod non_win_job {
    use tokio::process::Child;

    pub struct ChildJobGuard;

    impl ChildJobGuard {
        pub fn new() -> anyhow::Result<Self> {
            Ok(Self)
        }

        pub fn empty() -> Self {
            Self
        }

        pub fn assign_child(&self, _child: &Child) -> anyhow::Result<()> {
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use win_job::ChildJobGuard;

#[cfg(not(windows))]
pub use non_win_job::ChildJobGuard;

pub fn create_job_guard_best_effort() -> ChildJobGuard {
    match ChildJobGuard::new() {
        Ok(guard) => guard,
        Err(err) => {
            tracing::warn!("创建 Win32 Job 防孤儿守卫失败，将依赖显式停机清理: {err}");
            ChildJobGuard::empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_guard_initializes_cleanly() {
        let guard = ChildJobGuard::new().unwrap();
        drop(guard);
    }
}
