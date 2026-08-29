import { describe, expect, it } from "vitest";
import {
  boundedGatewayRows,
  gatewayChallenge,
  isPrivateTarget,
  MAX_GATEWAY_ROWS,
  emptyGatewayDraft,
} from "./model";

describe("gateway model", () => {
  it.each([
    "http://localhost:3000",
    "http://127.0.0.1",
    "https://10.0.0.2",
    "http://172.20.1.2",
    "http://192.168.1.2",
  ])("discloses private target %s", (url) => {
    expect(isPrivateTarget(url)).toBe(true);
  });

  it("discloses stdio, private-network, and OAuth behavior", () => {
    const draft = {
      ...emptyGatewayDraft(),
      transport: "stdio" as const,
      command: "npx",
      oauthEnabled: true,
    };
    const challenge = gatewayChallenge(draft);
    expect(challenge.stdio).toBe(true);
    expect(challenge.oauth).toBe(true);
    expect(challenge.messages).toHaveLength(2);
  });

  it("bounds list rendering", () => {
    expect(
      boundedGatewayRows(
        Array.from({ length: 150 }, (_, index) => ({
          id: String(index),
          source: "custom_gateway",
        })),
      ),
    ).toHaveLength(MAX_GATEWAY_ROWS);
    expect(boundedGatewayRows([{ id: "built-in", source: "in_process" }])).toEqual([]);
    expect(() => boundedGatewayRows({})).toThrow(/invalid gateway list/i);
  });
});
