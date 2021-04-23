// This module handles tracking state
//
// The state is conceptually an alternating series of nodes:
//
//  SessionNode corresponds to a session (that is, a set of processes bound to a tty)
//  GroupNode corresponds to a process group of a sesssion - we track tthe
//    foreground process group of each session
//
// Every SessionNode points to a GroupNode, but a GroupNode only points to a SessionNode
// if the process group contains a known TTY-forwarding process (toolbox, tmux, etc.)
// and we're able to track the TTY-forwarding to a new process.
//
// Types of changes that can happen:
//   * The foreground group of a SessionNode can change to a different foreground group
//   * A GroupNode can change from having no known SessionNode to having a known
//     SessionNode, and (less likely) vice-versa.

use crate::podman::{ContainerInfo, find_podman_peer};
use crate::process::Process;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct SessionNodeState {
    child: Option<Arc<GroupNode>>,
}

struct SessionNode {
    pid: i32,
    container_info: Option<ContainerInfo>,
    state: Mutex<SessionNodeState>,
}

impl SessionNode {
    fn new(pid: i32, container_info: Option<ContainerInfo>) -> Self {
        Self {
            pid,
            container_info,
            state: Mutex::new(SessionNodeState {
                child: None,
             })
        }
    }

    fn update(&self) {
        if let Ok(tty_pgrp) = Process::new(self.pid).tty_process_group() {
            let mut state = self.state.lock().unwrap();
            let changed = match &state.child {
                Some(arc) => tty_pgrp != arc.pgrp,
                None => true
            };
            if changed {
                state.child = Some(Arc::new(GroupNode::new(tty_pgrp)));
            }
        } else {
            let mut state = self.state.lock().unwrap();
            state.child = None
        }
    }

    fn child(&self) -> Option<Arc<GroupNode>> {
        let state = self.state.lock().unwrap();
        match &state.child {
            Some(arc) => Some(arc.clone()),
            None => None
        }
    }
}

struct GroupNodeState {
    child: Option<Arc<SessionNode>>,
}

struct GroupNode {
    pgrp: i32,
    state: Mutex<GroupNodeState>,
}

impl GroupNode {
    fn new(pgrp: i32) -> Self {
        Self { pgrp, state: Mutex::new(GroupNodeState { child: None }) }
    }

    fn update(&self) {
        let mut child_pid = -1;
        let mut container_info: Option<ContainerInfo> = None;
        if let Ok(argv0) = Process::new(self.pgrp).argv0() {
            if argv0 == "/home/otaylor/bin/toolbox" {
                if let Ok(peer) = find_podman_peer(self.pgrp) {
                    child_pid = peer.0;
                    container_info = peer.1;
                }
            }
        }

        if child_pid != -1 {
            let mut state = self.state.lock().unwrap();
            let changed = match &state.child {
                Some(arc) => child_pid != arc.pid,
                None => true
            };
            if changed {
                state.child = Some(Arc::new(SessionNode::new(child_pid, container_info)));
            }
        } else {
            let mut state = self.state.lock().unwrap();
            state.child = None
        }
    }

    fn child(&self) -> Option<Arc<SessionNode>> {
        let state = self.state.lock().unwrap();
        match &state.child {
            Some(arc) => Some(arc.clone()),
            None => None
        }
    }
}

pub struct TerminalState {
    root: Arc<SessionNode>,
    container_info: Mutex<Option<ContainerInfo>>,
    foreground_argv0: Mutex<String>,
    foreground_cwd: Mutex<PathBuf>,
}

impl TerminalState {
    pub fn new(root_pid: i32) -> Self {
        return TerminalState {
            root: Arc::new(SessionNode::new(root_pid, None)),
            container_info: Mutex::new(None),
            foreground_argv0: Mutex::new(String::from("")),
            foreground_cwd: Mutex::new(PathBuf::new())
        }
    }

    pub fn update(&self) {
        let mut session = self.root.clone();
        let mut leaf_group: Option<Arc<GroupNode>> = None;
        let mut container_session: Option<Arc<SessionNode>> = None;

        loop {
            if session.container_info.is_some() {
                container_session = Some(session.clone());
            }
            session.update();
            let group = match session.child() {
                Some(arc) => arc,
                None => break,
            };
            group.update();
            leaf_group = Some(group.clone());
            session = match group.child() {
                Some(arc) => arc,
                None => break,
            };
        }

        let mut foreground_argv0 = String::new();
        let mut foreground_cwd = PathBuf::new();

        if let Some(leaf_group) = leaf_group {
            let proc = Process::new(leaf_group.pgrp);
            if let Ok(argv0) = proc.argv0() {
                foreground_argv0 = argv0;
            }
            if let Ok(cwd) = proc.cwd() {
                foreground_cwd = cwd;
            }
        }

        let container_info = container_session.and_then(|x| x.container_info.clone());

        *(self.container_info.lock().unwrap()) = container_info;
        *(self.foreground_argv0.lock().unwrap()) = foreground_argv0;
        *(self.foreground_cwd.lock().unwrap()) = foreground_cwd;
    }

    pub fn container_info(&self) -> Option<ContainerInfo> {
        self.container_info.lock().unwrap().clone()
    }

    pub fn foreground_argv0(&self) -> String {
        {
            let foreground_argv0 = self.foreground_argv0.lock().unwrap();
            foreground_argv0.clone()
        }
    }

    pub fn foreground_cwd(&self) -> PathBuf {
        {
            let foreground_cwd = self.foreground_cwd.lock().unwrap();
            foreground_cwd.clone()
        }
    }
}

impl fmt::Display for TerminalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TerminalState[")?;
        let mut session = self.root.clone();
        loop {
            write!(f, " S-{}", session.pid)?;
            session.update();
            let group = match session.child() {
                Some(arc) => arc,
                None => break,
            };
            write!(f, " G-{}", group.pgrp)?;
            group.update();
            session = match group.child() {
                Some(arc) => arc,
                None => break,
            };
        }
        write!(f, " ]")
    }
}
