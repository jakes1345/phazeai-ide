//! Constructors for child processes that never open a console window.
//!
//! The desktop IDE is a GUI-subsystem program on Windows, so it has no console
//! of its own. Without `CREATE_NO_WINDOW`, every console program it starts
//! (git, ripgrep, language servers, debug adapters) would pop up a new,
//! empty console window for as long as it runs. On other platforms these are
//! plain `Command::new` calls.

use std::ffi::OsStr;

/// `CREATE_NO_WINDOW` from the Win32 process-creation flags.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `std::process::Command` for `program` that does not open a console window.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A `tokio::process::Command` for `program` that does not open a console window.
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_command_keeps_program() {
        assert_eq!(std_command("git").get_program(), "git");
    }

    #[test]
    fn tokio_command_keeps_program() {
        assert_eq!(tokio_command("git").as_std().get_program(), "git");
    }
}
