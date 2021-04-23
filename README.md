ttymon
======

ttymon is a process that acts as intermediate
between a terminal emulator and the child process running inside of it.
It uses whatever means necessary to retrieve information
about the process the user is interacting with inside the terminal.
This information includes:

* The identity (command line) of the process
* The current working directory of the process
* The container the process is running in

The information can then be sent back to the terminal emulator
via escape sequences or an extra file descriptor for out-of-band communication.

Motivation
----------

Implementing new features in terminal emulators
often requires knowledge about what is running in the terminal.
If we want to set a default title based on the foreground process in the terminal,
then we need to know what the foreground process is.
If we want to show the active git branch in the terminal,
then we need to know the working directory for the terminal.

One method of doing this
is to require the process inside the terminal to send escape sequences.
For example, the terminal escape sequence OSC 7 (`\e]7`)
can be used to send the working directory to the terminal.
But then all relevant clients need to be patched and properly configured.
For bash, the most common shell,
such escape sequences are done with
complicated and fragile setting of `$PROMPT_COMMAND`,  `$PS1` and so forth.

Another method is for the terminal to start from the process ID of its child
and try to figure things out by looking in /proc.
This avoids needing cooperation from the clients,
but runs into difficulty in more complicated cases.
What if the foreground process that the terminal finds is `tmux`, `podman exec -i`,
or `pipenv shell`?
In all of these cases,
the foreground process has created a new tty and is forwarding to that tty.

In such cases,
it is frequently possible to use heuristics and system-level APIs
to find the real foreground process,
but asking terminal emulators to embed that code into their applications
is unrealistic and, at best, would involve a lot of duplicated effort.
The goal of ttymon is to be a centralized place to do the heavy lifting.

Usage
-----

``` sh
ttymon --info-fd=3 bash -l
```

A stream of messages then can be read from fd 3.

Client-specific notes
---------------------

**podman** Support is limited to running podman in "rootless mode",
since ttymon needs permissions to see the containers processes.

**tmux** tmux provides the ability for a client to run in "control mode"
and get notifications about changes on the server.
Unfortunately,
such control mode clients are still clients that show up in the tmux user interface,
need to connect to a particular session,
and so forth.
(The mode was designed for terminals with native tmux support.)
The planned implementation tries to balance intrusiveness and efficiency,
but some enhancement to Tmux could be useful.

As a remote proxy
-----------------

There are places where ttymon can't follow the chain of clients -
to a different privilege domain, or to a different system.
In that cases, it should be possible to run ttymon on the remote system,
and have it send information back by escape sequence to the local ttymon.

Making this work automatically without extensive reconfiguration will be hard.

Terminals in Flatpaks
---------------------

Running terminal applications in a Flatpak can be tricky,
because the user doesn't want a shell inside the Flatpak sandbox -
they want a shell on the host (or in a different container).
The terminal can get around this with `flatpak-spawn --host`,
or by talking to the underlying D-Bus APIs that flatpak-spawn uses.
In that case, ttymon also needs to run on the host,
since it can't work inside a pid namespace.

In the spirit of ttymon handling the operating-specific low-level details,
ttymon could support a mode where it sets up the tty forwarding to the host
and re-execs a copy of itself on the host to handle monitoring.

Questions
---------

**Why not improve the existing escape-sequence ecosystem?**

Instead of doing ugly system-level scraping,
why not provide patches to clients to use existing escape sequences,
and define new escape sequences as necessary?

This is typically hard work -
e.g., [an effort to define a "current container" escape sequences](https://gitlab.freedesktop.org/terminal-wg/specifications/-/issues/17) has gone nowhere so far.

There are also "chicken and egg" problems -
new features in terminals won't work until client support is implemented,
but there is no incentive for clients to add support until there are terminals that use it.
ttymon can bootstrap things by getting things to work right away for many cases,
and if escape sequences that can be defined that make things work better,
or in more cases, then ttymon can support those too.

There is a tendendency for client support to be hidden behind configuration.
Authors of terminal clients consider things done
when it's *possible* for their users to set things up to work,
rather than when it just works for all users.

Finally, ttymon, in some ways, can just work better. Escape sequences lack context -
if an escape sequence shows a path to a current working directory,
what namespace is it in? Is it on the local system at all?

**Why is ttymon written in Rust?**

ttymon needs to both be as efficient as possible,
and also be secure when it parses the terminal stream and various files.
Rust is a good match for both of these.

**Why an executable, not a library?**

ttymon needs to do file descriptor IO, run timers, and so forth.
As a library,
it would need to be integrated with the application's event loop in a complicated way.
Making it an executable works with any event loop -
the application just reads a stream of events out of a file descriptor.
A separate executable also avoids
having to worry about interfacing Rust to the application language,
and a lot of details about sharing a process -
signal handlers, CLOEXEC flags, and so forth.

**Why does ttymon proxy the tty traffic?**

ttymon needs two things:
it needs to know when there is output from the tty,
since that is a good signal to know when it should be polling for changes.
It also needs to intercept OSC escape sequences,
since they can provide additional details, or fallbacks in cases where tracing fails.
Proxying the tty traffic provides a convenient way of achieving both goals.
It also allows ttymon to be integrated into the application
rather than the terminal emulation library when those are separate.

ttymon might eventually provide another mode of operation
where ttymon doesn't wrap the tty and
the application sends it high-level events like "tty traffic" or "osc sequence".

**Will ttymon be ported to operating systems other than Linux?**

ttymon extensively uses Linux system APIs like netlink and the `/proc` filesystem.
Porting to other operating systems would involve creating entirely new code paths
separate would be hard to keep tested and working.
Given sufficient demand,
and the availibility of CI testing on the target operating systems,
it might be an eventual possibility,
but the current goal is to work as well as possible on Linux.
