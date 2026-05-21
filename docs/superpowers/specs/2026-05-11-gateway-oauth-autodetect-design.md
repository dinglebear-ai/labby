# Design Spec: Gateway OAuth Auto-Detection and Auto-Connect

Status: Draft
Date: 2026-05-11
Topic: Gateway Admin UI Enhancement

## Overview

When adding a custom server to the Gateway, users currently have to manually select "OAuth" from a dropdown after entering a URL, even though the system already "probes" the URL in the background. This design automates that transition to provide a "magic" onboarding experience.

## Goals

- Automatically switch the Authentication mode to "OAuth (MCP)" when a URL is found to support it.
- Only perform this auto-switch if the user is currently in "No auth" mode (preserving intentional manual selections like Bearer tokens).
- Automatically attempt to open the OAuth authorization popup to save the user a click.
- Provide a clear fallback if the browser blocks the automatic popup.

## Architecture & Logic

### 1. Enhanced Auto-Probe Hook
The `useEffect` hook in `gateway-form-dialog.tsx` that handles URL probing will be extended.

- **Trigger:** URL change + successful OAuth probe result.
- **Condition:** `authMode === 'none'`.
- **Action:** 
    1. Call `setAuthMode('oauth')`.
    2. Trigger a new `handleAutoOauthConnect` function.

### 2. Auto-Connect Mechanism
Opening a popup automatically after an async network request (the probe) is likely to be blocked by modern browsers as it's not a direct result of a user click.

- **Strategy:** We will attempt `window.open`. 
- **Success:** If `window.open` returns a handle, we proceed with the authorization flow.
- **Blocked:** If it returns `null`, we update the `oauthState` to a new `blocked` kind.
- **UI Fallback:** When `oauthState.kind === 'blocked'`, the UI will display a prominent highlight on the "Connect via OAuth" button with a message: "OAuth detected! Click to authorize (popup was blocked)."

### 3. State Management
- Extend `OAuthConnectState` (in `lib/types/upstream-oauth.ts` or local to component) to include a `blocked` or `detected` state if necessary, or reuse `idle` with a "detected" flag.
- Ensure that the "probing" and "authorizing" states are handled correctly to prevent UI flickering.

## UI/UX Changes

- **Dropdown Transition:** The "Authentication" dropdown will visibly animate/change from "No auth" to "OAuth (MCP)".
- **Feedback:** A brief "Detecting OAuth..." status message will appear.
- **Button State:** If auto-open fails, the "Connect via OAuth" button will become the primary focus of the form section.

## Testing Plan

1. **Happy Path:** Enter a URL known to support OAuth. Verify mode switches to OAuth and popup opens.
2. **Blocked Path:** Enter a URL in a browser configured to block popups. Verify mode switches and a clear "Click to authorize" message appears.
3. **Preservation Path:** Manually select "Bearer token", then enter an OAuth-supporting URL. Verify the mode stays as "Bearer token".
4. **Edit Path:** Edit an existing server. Verify auto-detection doesn't disrupt existing saved configurations unless the URL is changed.

## Future Considerations
- Supporting auto-detection for other auth types if they advertise themselves (e.g., via specialized headers).
