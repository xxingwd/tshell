# Docker workspace

Docker is a left-sidebar tool following the shared host selector: local host selects
its local Engine and a saved SSH host selects its remote Engine. It works without
terminal sessions. Selecting a host on this page does not start a shell or tmux.
Each host/endpoint retains resource selections, searches, scroll positions, detail
modes, operation output and file drafts for the application lifetime.

## Navigation and resources

The sidebar has four tabs: **Containers / Compose / Images / More**. It uses a
search field, a creation menu and refresh, without another Docker title, status
filters or resource counters. Narrow sidebars use icons with tooltips. Lists are
virtualized; arrow keys, Home and End move the selection. The main pane has one
resource toolbar, a native preview/editor and the existing status bar.

| Tab | Preview | Actions |
| --- | --- | --- |
| Containers | Overview, logs, inspect JSON; limits and a resource-usage sample; linked image, volume and network | Create, start/stop, restart, pause/resume, rename, update runtime parameters, recreate, remove, terminal |
| Compose | Projects and services, source YAML and command output | Import/associate files, validate, save/restore, up/start/stop/restart, pull, build, logs, down; optional service scope |
| Images | Tags, size, layers/history, referencing containers and raw JSON | Pull, tag, push, remove, import/export, create a container |
| More | Engine information, disk usage, volumes, networks, events and daemon configuration | Volume/network create/remove, network connect/disconnect, categorized pruning, daemon validation/save/restore/reload/restart |

Container and Compose navigation share one container snapshot. Compose grouping
uses the exact project/service labels, not name heuristics. Image identity uses the
complete ID even when the sidebar displays a short untagged ID. Related resource
links stay inside Docker and preserve the previous page's browsing state.

## Connections and requirements

`tshell-docker` uses Bollard **0.21.1** directly for Engine models, version
negotiation, API calls, events, logs and attached exec. Windows defaults to
`//./pipe/docker_engine`; Unix defaults to `/var/run/docker.sock`. More → Engine
information provides the local endpoint setting; an empty path restores the
platform default. The explicit endpoint is persisted in `workspace.json`.
Local SDK operations, including streamed archive import/export, need an accessible
running Engine but no Docker CLI. They do not inherit local Docker contexts or
`DOCKER_HOST`. Image pull currently uses anonymous registry access.

SSH uses TShell's native russh authentication and connection pools, running
`docker system dial-stdio` without a PTY on the selected SSH host. The remote host
needs Docker CLI and permission to access its Engine. Its selected endpoint/context
is captured when connecting. No external local SSH executable, forwarded socket,
remote Python or exposed daemon TCP port is needed. HTTP stderr is isolated from
protocol bytes; attached exec uses the terminal pool rather than the query pool.

Compose, image push and Docker Desktop restart use the Docker CLI on the selected
host; Compose additionally needs its plugin. Push uses that host's existing CLI
credential helpers. CLI mutations pin an explicit endpoint/context and check Engine
ID against the SDK snapshot before execution. Remote archives stay on the remote
filesystem. Docker context/TLS profile management is not a separate TShell UI.

## Operations and container terminals

Forms and confirmation dialogs capture the original host, endpoint and complete
resource identity. A changed connection or different Engine ID rejects a mutation.
Operations continue on their original target while browsing another host; results
remain in that target's preview output. SDK pulls and archives report progress;
CLI commands show their bounded output when they finish. Uncertain network outcomes
are not replayed automatically. Events coalesce snapshot refreshes.

Container removal requires a stopped container, never forces removal and does not
delete volumes. Recreate requires a stopped container and editable creation JSON:
the old container is renamed to a unique backup name, a stopped replacement is
created under the original name, and the backup is retained. Failed creation tries
to restore the original name and reports any rollback failure. This is deliberately
separate from updating the Engine-supported runtime parameters.

Logs request the last 1,000 lines and follow new output. Pause cancels the stream;
resume requests a fresh tail. Search, copy and auto-scroll use a read-only native
editor. Logs retain at most **2 MiB and 10,000 newline separators** and preserve
UTF-8 across chunks. Operation output retains at most **256 KiB** and events retain
500 entries. Stats are a one-shot sample, not a continuously polled CPU dashboard.
Hiding Docker cancels browsing, detail, events and log workers while mutations and
existing terminals keep their original target.

The terminal button creates Bollard attached TTY exec using `/bin/sh` for Linux or
`cmd.exe` for Windows; users can edit the command arguments. Input/output uses the
existing terminal parser/renderer and resize uses the Engine API. Terminals have
explicit `Docker · Engine · container` session names. The same endpoint/container
can add tabs to the same temporary session; different sources remain separate.
These sessions are not restored as ordinary shells. Closing exec does not stop or
remove the container. Existing terminals never retarget on host switches, and
Explorer/Git remain host tools rather than container filesystem browsers.

## Compose files

Projects are discovered from container labels and, when available,
`docker compose ls --all --format json`. Imported source files also identify
stopped projects; imports are currently retained for the app lifetime. Missing CLI
or inaccessible source files do not prevent container/label browsing.

Projects bind a name, ordered configuration files and working directory. The file
selector edits individual original YAML files; each file has its own draft. Source
paths from labels may belong to another deployment machine and must be accessible
on the selected host, or associated explicitly. No expanded YAML overwrites source.

Validation uses official `compose config --quiet`, substituting a temporary edited
file in its original position while retaining the other files and project directory.
CLI owns interpolation, merge, dependencies and build semantics. Down and validation
always apply to the whole project; service selection scopes compatible operations.

## daemon.json

More → Engine configuration edits JSON directly. Detection uses actual Engine and
host evidence: local Docker Desktop, native Windows default-pipe service, Linux
system/rootless service and its `--config-file`, or an unambiguous dockerd process.
An SSH context pointing at another daemon is not assumed to own local daemon files.
Custom/proxy/ambiguous installations need explicit source association. A manually
associated file can be read/saved, but cannot control an unverified service.

Saving and applying are separate actions. JSON must be an object, unknown keys are
retained, and native dockerd validation is used when available. Desktop or missing
dockerd gets clearly labelled JSON-only validation. Dockerd validation alone does
not prove compatibility with conflicting service flags or that every setting can
be reloaded.

Files have a 2 MiB limit. Local files and remote SFTP files reject symlinks; saves
compare the previously loaded content, refuse external changes, create a backup
and replace via an adjacent temporary file. Existing permissions are preserved;
new files use restrictive permissions where supported. Restore also checks for
external changes. Drafts survive page/host switches; the discard icon confirms reverting the current
file to its loaded/saved content without writing to disk. Host deletion or connection
changes are blocked while there are dirty drafts or running operations.

Configuration files can be repaired while the API is offline. Reload/restart needs
a verified live Engine/service binding, a saved valid file and the required account
permissions. Reload is offered only for a service reporting support and applies
only settings Docker can reload. Restart is explicit. The operation waits for the
same Engine ID to return; this confirms API recovery, not that every configuration
value has taken effect. Offline service startup must be handled externally.

Filesystem and service operations run with the current local/SSH account. Permission
failures are shown; no automatic sudo/elevation or privileged credential workflow
is implemented. Desktop uses public daemon JSON and CLI, not internal settings files.

## Ownership and validation

The application owns host navigation, preferences and terminal attachment.
`tshell-docker` owns resource state, drafts, native controls and asynchronous work.
Explorer, Git and Docker remain separate crates using concrete `tshell-host` and
`tshell-ui` capabilities; there is no generic resource/plugin registry or SDK wrapper.

Workspace tests cover target identity, SDK request payloads, archive streaming,
filesystem conflicts/backups/restoration, offline configuration saves, bounded logs,
HTTP upgrade/exec and loopback SSH channel/stderr behavior. Native Windows checks
exercise 10,000 containers/images, all four pages, YAML/JSON/output, dark/light and
narrow layouts, keyboard navigation, sidebar collapse, draft restoration, stale
responses and host switching without terminal sessions.

The development machine has no Docker installation or `TSHELL_SSH_TEST_HOST`.
Protocol/UI fixtures have passed; real Engine/Compose/daemon/SFTP permissions,
service recovery, registry credentials, platform-specific source detection and
Windows-container exec still need environment acceptance. Linux/macOS native
startup remains outside continuous verification. See the
[design record](docker-resource-design.md) and [implementation checklist](../todo.md#9-docker-资源界面与管理功能).
