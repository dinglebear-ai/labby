import { dispatchAction, type PaletteResult, resultErrorMessage } from "@/lib/labbyClient";
import {
  boundedGatewayRows,
  type GatewayDraft,
  type GatewayRuntime,
  type GatewayView,
  gatewayFingerprint,
  MAX_GATEWAY_ARGS,
  MAX_GATEWAY_PATTERNS,
} from "./model";

export class GatewayClientError extends Error {
  constructor(
    message: string,
    readonly kind = "gateway_error",
    readonly status = 0,
  ) {
    super(message);
    this.name = "GatewayClientError";
  }
}

export class StaleGatewayError extends GatewayClientError {
  constructor() {
    super(
      "Gateway state changed on the server. Refresh and review the current values before retrying.",
      "stale_state",
      409,
    );
  }
}

async function action<T>(name: string, params: object): Promise<T> {
  const result = await dispatchAction("gateway", name, params);
  if (!result.ok) throw toGatewayError(result);
  return result.payload as T;
}

function toGatewayError(result: PaletteResult): GatewayClientError {
  const payload = result.payload as Record<string, unknown> | null;
  const kind = payload && typeof payload.kind === "string" ? payload.kind : "gateway_error";
  return new GatewayClientError(resultErrorMessage(result), kind, result.status);
}

function lines(value: string, max: number): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(0, max);
}

function specFromDraft(draft: GatewayDraft) {
  return {
    name: draft.name.trim(),
    enabled: draft.enabled,
    url: draft.transport === "http" ? draft.url.trim() : null,
    command: draft.transport === "stdio" ? draft.command.trim() : null,
    args: draft.transport === "stdio" ? lines(draft.args, MAX_GATEWAY_ARGS) : [],
    bearer_token_env: draft.bearerTokenEnv.trim() || null,
    oauth: draft.oauthEnabled
      ? {
          mode: "authorization_code_pkce",
          registration: { strategy: "dynamic" },
          scopes: null,
        }
      : null,
    proxy_resources: draft.proxyResources,
    proxy_prompts: draft.proxyPrompts,
    expose_tools: draft.exposeTools.trim() ? lines(draft.exposeTools, MAX_GATEWAY_PATTERNS) : null,
  };
}

async function assertCurrent(name: string, expectedFingerprint: string): Promise<void> {
  const current = await gatewayClient.get(name);
  if (gatewayFingerprint(current) !== expectedFingerprint) throw new StaleGatewayError();
}

export const gatewayClient = {
  async list(): Promise<GatewayView[]> {
    const rows = boundedGatewayRows(await action<unknown>("gateway.list", {}));
    const views: GatewayView[] = [];
    // Keep detail fanout bounded so a large gateway cannot create an unbounded
    // burst of admin requests or retained render state.
    for (let offset = 0; offset < rows.length; offset += 6) {
      views.push(
        ...(await Promise.all(
          rows.slice(offset, offset + 6).map((row) => gatewayClient.get(row.id)),
        )),
      );
    }
    return views;
  },

  get(name: string): Promise<GatewayView> {
    return action("gateway.get", { name });
  },

  testDraft(draft: GatewayDraft): Promise<GatewayRuntime> {
    return action("gateway.test", { spec: specFromDraft(draft) });
  },

  testSaved(name: string): Promise<GatewayRuntime> {
    return action("gateway.test", { name });
  },

  create(draft: GatewayDraft): Promise<GatewayView> {
    return action("gateway.add", { spec: specFromDraft(draft) });
  },

  async update(
    name: string,
    draft: GatewayDraft,
    expectedFingerprint: string,
  ): Promise<GatewayView> {
    await assertCurrent(name, expectedFingerprint);
    return action("gateway.update", {
      name,
      expected_revision: expectedFingerprint,
      patch: specFromDraft(draft),
    });
  },

  async remove(name: string, expectedFingerprint: string): Promise<void> {
    await assertCurrent(name, expectedFingerprint);
    await action("gateway.remove", { name, expected_revision: expectedFingerprint });
  },

  async reload(name: string, expectedFingerprint: string): Promise<GatewayView> {
    await assertCurrent(name, expectedFingerprint);
    await action("gateway.reload", { name, expected_revision: expectedFingerprint });
    return gatewayClient.get(name);
  },
};
