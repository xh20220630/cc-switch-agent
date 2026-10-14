import { describe, expect, it } from "vitest";
import { http, HttpResponse } from "msw";
import { providersApi } from "@/lib/api/providers";
import { setRuntimeSnapshot } from "@/lib/runtime/store";
import { server } from "../msw/server";

describe("交换 API 的目标隔离", () => {
  it("当前在远程 A，交换仍固定写入指定的远程 B，无须切换 Runtime", async () => {
    setRuntimeSnapshot({
      status: "online",
      generation: 9,
      activeTargetId: "remote-a",
    });
    const calls: Record<string, unknown>[] = [];
    server.use(
      http.post(
        "http://tauri.local/remote_invoke_provider_target",
        async ({ request }) => {
          const payload = (await request.json()) as Record<string, unknown>;
          calls.push(payload);
          return HttpResponse.json(
            payload.command === "provider.list" ? {} : true,
          );
        },
      ),
    );
    const copy = { id: "copy", name: "Copy", settingsConfig: {} };
    await providersApi.listExchangeTarget("codex", "remote-b");
    await providersApi.addExchangeCopy(copy, "codex", "remote-b");
    expect(calls).toEqual([
      {
        targetId: "remote-b",
        command: "provider.list",
        args: { app: "codex" },
      },
      {
        targetId: "remote-b",
        command: "provider.add",
        args: { app: "codex", provider: copy, addToLive: false },
      },
    ]);
    setRuntimeSnapshot({ status: "local", generation: 10 });
  });
  it("本机副本使用待启用接口，不随当前远程模式转发", async () => {
    setRuntimeSnapshot({
      status: "online",
      generation: 12,
      activeTargetId: "remote-a",
    });
    let payload: unknown;
    server.use(
      http.post("http://tauri.local/add_provider_copy", async ({ request }) => {
        payload = await request.json();
        return HttpResponse.json(true);
      }),
    );
    const provider = { id: "copy", name: "Copy", settingsConfig: {} };
    await providersApi.addExchangeCopy(provider, "claude");
    expect(payload).toEqual({ app: "claude", provider });
    setRuntimeSnapshot({ status: "local", generation: 13 });
  });
});
