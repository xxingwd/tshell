# SSH port forwarding

The forwarding icon in the bottom status bar opens one global list, regardless of the active
terminal host. Every row shows its type, SSH host, listen/target ports, and status.
There is no forwarding entry in the host menu. The dialog shows the table and
an **Add** action, without a saved-rule count or explanatory empty-state caption.
The status-bar entry is an icon with a tooltip; it has no aggregate count or byte summary.

**Add** inserts an editable row into the same list. Select a forwarding
type and a saved SSH host, enter the ports, then save. SOCKS5 has only a listening
port. Save starts the rule immediately. Cancel removes the unsaved row. Existing
rows offer start, stop, and delete actions; deleting a row leaves other tunnels
running. Closing the dialog and switching terminal hosts do not restart stopped
rules or interrupt running forwards.
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
status icon, global list and inline add row.
`--workspace-ui-check` also clicks Add, Cancel/Save, then the dialog's close
button in both desktop sizes to verify focus recovery. The key under
`tests/fixtures/ssh-test-host-key` is a public test-only fixture.
