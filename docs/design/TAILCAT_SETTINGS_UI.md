# Native Tailcat setup panel

The Settings panel prepares dashboard connectivity on the existing native OAuth
gateway. It uses the configured operator's browser session and selected project;
the server remains authoritative for project ownership and prerequisites.

The panel has three explicit steps: preview/save restricted configuration,
enroll a project credential into private native custody, and enable the controller.
It displays exact assignment and restart requirements from backend responses.
It neither changes project assignments nor starts or manages virtual machines.

Inputs use existing Aurora controls and SettingsChrome sections. Status and
structured errors are accessible text. Enrollment keys remain in component memory
for retries; project or authority changes abort work and clear all previous
results. Native setup requests omit Team/project routing headers while preserving
session cookies, CSRF, cancellation and response authority checks. Other callers
retain the standard routing headers.

After a gateway restart or page refresh, enrollment can resume against the live
server policy without reconfiguring. Activation can resume using the public
credential identifier from a previous enrollment. Client step history is never
an authority gate. Editing configuration fields clears stale displayed results.

The credential response exposes only an identifier and private host file path.
The panel never receives or stores the credential bearer. Pairing remains a local
CLI approval using that path. The route is a static Next.js client page, with
stacked controls that remain usable on narrow screens.
