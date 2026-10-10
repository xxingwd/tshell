# Docker workspace

Docker is a left-sidebar tool that follows the workspace's shared host selector.
The local host selects the local Engine; a saved SSH host selects its remote Engine.
Switching sessions within a host, closing every terminal, or hiding the sidebar
does not change that host's Engine. Selecting a host on the Docker page does not
start a shell or tmux session. The header and status bar show the actual endpoint.
The explicit local endpoint is saved in `workspace.json`; each Engine retains its
own container selection, filters, detail tab and logs during the app lifetime.
The previous independent `docker_target` preference is ignored when loading.

## Connections

`tshell-docker` uses Bollard **0.21.1** directly for version negotiation, models,
container requests, events, logs and exec. Local Windows connections use
`//./pipe/docker_engine`; Unix connections use `/var/run/docker.sock`. The endpoint
button accepts a different absolute pipe/socket path; an empty value restores the
default. Local access needs a running Engine and permission to access its endpoint.
It does not require a local Docker CLI or inherit `DOCKER_HOST`/Docker contexts.

Every saved SSH host is also an Engine target, including disconnected terminal hosts
and hosts configured for tmux. SSH uses TShell's native russh authentication and
connection pools. Each client retains its auxiliary pool lease across API requests,
so negotiation, inspection and refresh reuse authentication and the transport.
A small HTTP transport runs **`docker system dial-stdio` on the
remote host**, without a PTY. Thus the remote host needs the Docker CLI, a running
Engine and permission to access it. No daemon TCP port, external local SSH process,
remote Python, mounted filesystem or forwarded Docker socket is required.

SSH stderr is kept outside HTTP/exec bytes, bounded separately and reported when
the command fails. HTTP requests use origin-form paths; Hyper preserves upgraded
exec streams. Exec uses the terminal connection pool so long-lived terminals do
not exhaust the auxiliary pool used by queries. Changing host connection fields
invalidates that Engine's browsing client; existing terminals retain their original
connection. Host display-name changes retain identity and browsing state.

## Containers and logs

The virtual container list supports text search, running/stopped filters and Compose
project grouping. The main view shows state, image, full ID, health, timestamps,
ports and mounts, alongside read-only raw inspect JSON.

Start, stop, restart and removal prompts capture the original Engine and complete
container ID. Confirmation rechecks the target configuration and container state.
Removal requires a stopped container and never forces removal or deletes volumes.
Results and failures identify the source even after switching Engines. Each operation
reads the actual list afterward; uncertain network outcomes are never replayed
automatically. Container events coalesce refreshes; reconnecting the events stream
also requests a complete snapshot.

Logs initially request the last 1,000 lines and follow new output. Pause cancels the
stream; resume starts a fresh 1,000-line tail. Search, copy and automatic scrolling
operate on a read-only native editor. The buffer retains at most **2 MiB and 10,000
newline separators**, including oversized chunks; UTF-8 split across chunks is
preserved. A bounded message queue batches UI updates. Hiding the page cancels its
query, detail, event and log workers; existing mutation operations and terminals
keep their original targets.

## Container terminals

The terminal button creates an attached TTY exec through Bollard. The command
starts as `/bin/sh` for Linux or `cmd.exe` for Windows and can be edited as an
argument list. Missing commands or stopped containers report an error.

Each exec feeds TShell's existing terminal parser, renderer and input contract;
resize calls the Engine exec API. Resize updates are coalesced without being lost
when the input queue is full, and applied before subsequently forwarded input.
Terminals appear in local navigation with an explicit `Docker · Engine · container`
name. Reopening the same Engine endpoint/container adds another tab to that
container's session; different Engine sources remain separate.

These sessions are temporary and are not restored as shell sessions at startup.
Open extra container terminals from the Docker page; ordinary shell tabs and splits
are unavailable inside a container session. Explorer/Git and terminal file links
remain host tools and do not access a container filesystem. Closing a terminal
cancels its attached exec connection; it does **not** stop/remove the container.
Switching hosts never retargets an existing container terminal.

## Ownership and validation

The application owns navigation, preferences and terminal attachment. `DockerView`
owns Engine clients, list/detail state, workers and native controls. Explorer, Git
and Docker are separate crates with no mutual dependencies. They share concrete
`tshell-host` and `tshell-ui` capabilities, not a generic tool/plugin interface.

Automated checks cover bounded Unicode logs, Bollard version negotiation and raw
HTTP upgrade, attached exec input/resize/drop, rejection of stopped-container exec,
and a loopback russh fixture proving stderr isolation and channel cancellation.
Native Windows checks exercise 10,000 containers, bounded logs, inspect/overview,
dark/light themes, narrow layouts, sidebar collapse, delayed-response rejection and
host-following while the Docker page is visible or hidden, per-Engine browsing
restoration, same-host session changes and operation with no terminal sessions.
These are protocol/UI fixtures.

The development machine used for this change has no Docker installation and no
configured `TSHELL_SSH_TEST_HOST`. Real local/remote Engine permissions, long-running
logs, external deletion, missing shells, disconnect outcomes and Windows-container
exec still need environment acceptance. Linux/macOS native startup remains outside
continuous verification. Image/volume/network management, stats, Compose execution,
Docker contexts and custom TLS endpoints are future work.
