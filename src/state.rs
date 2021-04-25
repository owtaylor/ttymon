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

use crate::podman::{find_podman_peer, ContainerInfo};
use crate::process::Process;
use std::fmt;
use std::path::{Path, PathBuf};

struct SessionNode {
    pid: i32,
    container_info: Option<ContainerInfo>,
    child: Option<Box<GroupNode>>,
}

impl SessionNode {
    fn new(pid: i32, container_info: Option<ContainerInfo>) -> Self {
        Self {
            pid,
            container_info,
            child: None,
        }
    }

    fn update(&mut self) {
        if let Ok(tty_pgrp) = Process::new(self.pid).tty_process_group() {
            let changed = match &self.child {
                Some(group) => tty_pgrp != group.pgrp,
                None => true,
            };
            if changed {
                self.child = Some(Box::new(GroupNode::new(tty_pgrp)));
            }
        } else {
            self.child = None
        }
    }

    fn child(&self) -> Option<&GroupNode> {
        self.child.as_deref()
    }

    fn child_mut(&mut self) -> Option<&mut GroupNode> {
        self.child.as_deref_mut()
    }
}

struct GroupNode {
    pgrp: i32,
    child: Option<Box<SessionNode>>,
}

impl GroupNode {
    fn new(pgrp: i32) -> Self {
        Self { pgrp, child: None }
    }

    fn update(&mut self) {
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
            let changed = match &self.child {
                Some(session) => child_pid != session.pid,
                None => true,
            };
            if changed {
                self.child = Some(Box::new(SessionNode::new(child_pid, container_info)));
            }
        } else {
            self.child = None
        }
    }

    fn child(&self) -> Option<&SessionNode> {
        self.child.as_deref()
    }

    fn child_mut(&mut self) -> Option<&mut SessionNode> {
        self.child.as_deref_mut()
    }
}

pub struct TerminalState {
    root: SessionNode,
    container_info: Option<ContainerInfo>,
    foreground_argv0: String,
    foreground_cwd: PathBuf,
}

impl TerminalState {
    pub fn new(root_pid: i32) -> Self {
        return TerminalState {
            root: SessionNode::new(root_pid, None),
            container_info: None,
            foreground_argv0: String::from(""),
            foreground_cwd: PathBuf::new(),
        };
    }

    pub fn update(&mut self) {
        self.root.update();
        let mut group = match self.root.child_mut() {
            Some(group) => group,
            None => {
                self.container_info = None;
                self.foreground_argv0 = String::new();
                self.foreground_cwd = PathBuf::new();

                return;
            }
        };

        let mut group_pgrp: i32;
        let mut container_info: Option<ContainerInfo> = None;

        loop {
            group_pgrp = group.pgrp;
            group.update();
            let session = match group.child_mut() {
                Some(session) => session,
                None => break,
            };

            session.update();
            container_info = session.container_info.clone();
            group = match session.child_mut() {
                Some(group) => group,
                None => break,
            };
        }

        let proc = Process::new(group_pgrp);
        self.foreground_argv0 = proc.argv0().unwrap_or(String::new());
        self.foreground_cwd = proc.cwd().unwrap_or(PathBuf::new());
        self.container_info = container_info;
    }

    pub fn container_info(&self) -> Option<&ContainerInfo> {
        self.container_info.as_ref()
    }

    pub fn foreground_argv0(&self) -> &str {
        self.foreground_argv0.as_str()
    }

    pub fn foreground_cwd(&self) -> &Path {
        self.foreground_cwd.as_path()
    }
}

impl fmt::Display for TerminalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TerminalState[")?;
        let mut session = &self.root;
        loop {
            write!(f, " S-{}", session.pid)?;
            let group = match session.child() {
                Some(group) => group,
                None => break,
            };
            session = match group.child() {
                Some(session) => session,
                None => break,
            };
        }
        write!(f, " ]")
    }
}
