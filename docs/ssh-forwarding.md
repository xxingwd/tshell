# SSH port forwarding

The forwarding icon in the bottom status bar opens one global list, regardless of the active
terminal host. Every row shows its type, SSH host, separate listen and target port
columns, and status. Rows without a target port leave that column empty; SOCKS5
drafts also omit its input while preserving column alignment.
There is no forwarding entry in the host menu. The compact dialog uses 36-pixel
rows and a **+** icon in the table header. Its body grows with the number of rules,
up to 560 pixels or the available window height, with remaining rows scrollable.
Validation messages share that scroll area without a permanent empty footer;
they do not change the dialog height or header position.
The status-bar entry is an icon with a tooltip; it has no aggregate count or byte summary.

**+** inserts an editable row into the same list. Select a forwarding
type and a saved SSH host, enter the ports, then confirm with **✓** or Enter.
SOCKS5 has only a listening port. Confirmation starts the new rule immediately;
**×** discards the draft. Existing rows offer start/stop, edit, and delete icons,
all with localized tooltips and accessible names. Editing replaces that row with
the same controls used for adding. Cancelling preserves the rule and its tunnel;
confirming an unchanged rule also preserves its running/stopped state. Changed
rules restart individually and retain their saved listen/target addresses.
Changing the SSH host moves the rule to that host. Other row actions are disabled
while a draft is open. Closing the dialog and switching terminal hosts do not
restart stopped rules or interrupt running forwards.
Saving or cancelling an add row returns focus to the dialog; its close button
and keyboard dismissal remain available after the port inputs disappear.

All saved rules start once when TShell launches, using their SSH host's existing
authentication and known-host verification. They do not require an open terminal
session. A failed rule waits for an explicit Start action. There is no background
automatic reconnect. Deleting its SSH host also removes its rules.

Supported modes, with loopback listeners by default:

| Type | Listening endpoint | Target endpoint |
| --- | --- | --- |
| Local | This computer's `127.0.0.1:<listen>` | SSH host's `localhost:<target>` |
| Remote | SSH host's `127.0.0.1:<listen>` | This computer's `localhost:<target>` |
| SOCKS5 | This computer's `127.0.0.1:<listen>` | CONNECT destination requested by the proxy client |

SOCKS5 supports IPv4, IPv6, and domain destinations. Forwarding uses native
`russh` channels. The saved SSH host supplies credentials; its name is displayed
in the global list. Previously saved rules retain their addresses and ports.
Payload counter details and failure messages are available on the status tooltip.

OSC 52 clipboard copying remains enabled by default with no SSH-specific setting.
Validation includes native loopback SSH fixtures for all three modes, payload
counters and cancellation, plus dark and narrow light desktop UI captures of the
status icon, global list and inline add/edit rows.
`--workspace-ui-check` also clicks the add and cancel/confirm icons, then the
dialog's close button in both desktop sizes to verify focus recovery. It checks
invalid edits, cancellation, unchanged stopped rules, endpoint preservation,
persistence and moving a rule between hosts. The key under
`tests/fixtures/ssh-test-host-key` is a public test-only fixture.
