#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

mod filter;
mod podman;
mod process;
mod pty;
mod socket;
mod state;

use pty::{Pty, PtyActions};
use state::TerminalState;
use std::path::PathBuf;

struct Actions {
    home: PathBuf,
    state: TerminalState,
}

impl Actions {
    fn new(child_pid: i32) -> Actions {
        Actions {
            home: dirs::home_dir().unwrap(),
            state: TerminalState::new(child_pid),
        }
    }
}

impl PtyActions for Actions {
    fn check(&mut self) {
        self.state.update();
    }

    fn make_window_title(&self, in_window_title: &str) -> String {
        let container_info = self.state.container_info();
        let container_string = match container_info {
            Some(ci) => format!("{} - ", ci.container_name),
            None => String::from(""),
        };

        let mut foreground_cwd = PathBuf::from(self.state.foreground_cwd());
        if let Ok(home_suffix) = foreground_cwd.strip_prefix(&self.home) {
            foreground_cwd = PathBuf::from("~").join(home_suffix);
        }

        let foreground_argv = self.state.foreground_argv0();

        format!(
            "{}{} - {} - {}",
            container_string,
            foreground_cwd.to_string_lossy(),
            foreground_argv,
            in_window_title
        )
    }
}

fn main() {
    env_logger::init();

    let mut pty = match Pty::new() {
        Ok(pty) => pty,
        Err(e) => {
            error!("Failed to create: {}", e);
            std::process::exit(1);
        }
    };

    let child_pid = match pty.fork() {
        Ok(pid) => pid,
        Err(e) => {
            error!("Failed to fork subprocess: {}", e);
            std::process::exit(1);
        }
    };

    let mut actions = Actions::new(child_pid as i32);

    match pty.handle(&mut actions) {
        Ok(()) => {}
        Err(e) => {
            error!("Failed to handle IO with subprocess: {}", e);
            std::process::exit(1);
        }
    }
}
