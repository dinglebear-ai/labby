// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const callbacks = vi.hoisted(() => ({ changed: null as null | (() => void) }));
const oauth = vi.hoisted(() => ({ status: vi.fn(), login: vi.fn(), logout: vi.fn() }));

vi.mock("@/lib/invoke", () => ({
  appWindow: {
    listen: (_event: string, callback: () => void) => {
      callbacks.changed = callback;
      return Promise.resolve(() => {});
    },
  },
}));

vi.mock("@/lib/oauthClient", async () => {
  const actual = await vi.importActual<typeof import("@/lib/oauthClient")>("@/lib/oauthClient");
  return {
    ...actual,
    oauthStatus: oauth.status,
    oauthLogin: oauth.login,
    oauthLogout: oauth.logout,
  };
});

import { useOauthSession } from "./useOauthSession";
import type { OauthStatus } from "./oauthClient";

const signedOut: OauthStatus = {
  signedIn: false,
  scope: null,
  expiresAtUnix: null,
  serverUrl: null,
};
const signedIn: OauthStatus = {
  signedIn: true,
  scope: "lab:read",
  expiresAtUnix: 4_102_444_800,
  serverUrl: "https://lab.example.com",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  callbacks.changed = null;
});

it("ignores an older status request that resolves after a newer event load", async () => {
  const older = deferred<OauthStatus>();
  const newer = deferred<OauthStatus>();
  oauth.status.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
  const { result } = renderHook(() => useOauthSession());

  await waitFor(() => expect(callbacks.changed).toBeTypeOf("function"));
  act(() => callbacks.changed?.());
  await act(async () => newer.resolve(signedIn));
  await waitFor(() => expect(result.current.status).toEqual(signedIn));
  await act(async () => older.resolve(signedOut));
  expect(result.current.status).toEqual(signedIn);
});

it("ignores an in-flight mount read after sign-in completes", async () => {
  const mount = deferred<OauthStatus>();
  oauth.status.mockReturnValueOnce(mount.promise);
  oauth.login.mockResolvedValueOnce(signedIn);
  const { result } = renderHook(() => useOauthSession());

  await act(async () => result.current.signIn());
  expect(result.current.status).toEqual(signedIn);
  await act(async () => mount.resolve(signedOut));
  expect(result.current.status).toEqual(signedIn);
});
