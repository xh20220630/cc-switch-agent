import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { http, HttpResponse } from "msw";
import { ProviderList } from "@/components/providers/ProviderList";
import { useProviderActions } from "@/hooks/useProviderActions";
import { useProvidersQuery } from "@/lib/query/queries";
import { setRuntimeSnapshot } from "@/lib/runtime/store";
import type { Provider } from "@/types";
import { server } from "../msw/server";

// 保留真实列表、行按钮、动作 hook 和远程 API 适配，只在 IPC 边界用虚构的远端库响应。
// 这样能发现「单张卡片有运行按钮，但远程完整列表分支没把它传下来」的接线回归。
afterEach(() => {
  cleanup();
  setRuntimeSnapshot({ status: "local", generation: 0 });
});

it("switches a remote provider through the real row and refreshes its current state", async () => {
  const providers: Record<string, Provider> = {
    a: { id: "a", name: "Remote A", settingsConfig: {} },
    b: {
      id: "b",
      name: "Remote B",
      settingsConfig: {},
      meta: { apiFormat: "openai_responses" },
    },
  };
  let current = "a";
  const switched = vi.fn();
  const localSwitch = vi.fn();
  const exchange = vi.fn();
  server.use(
    http.post("http://tauri.local/remote_invoke", async ({ request }) => {
      const payload = (await request.json()) as {
        command: string;
        args: { app: string; id?: string };
        generation: number;
      };
      expect(payload.generation).toBe(77);
      expect(payload.args.app).toBe("claude");
      switch (payload.command) {
        case "provider.list":
          return HttpResponse.json(providers);
        case "provider.current":
          return HttpResponse.json(current);
        case "provider.switch":
          switched(payload);
          current = payload.args.id!;
          return HttpResponse.json({ warnings: [] });
        default:
          return HttpResponse.json(
            { error: "Unexpected remote command" },
            { status: 400 },
          );
      }
    }),
    http.post("http://tauri.local/switch_provider", () => {
      localSwitch();
      return HttpResponse.json({ warnings: [] });
    }),
    // 卡片还会读取插件状态；这些桌面元数据在测试中固定为空，不连接真实客户端。
    http.post("http://tauri.local/get_current_omo_provider_id", () =>
      HttpResponse.json(null),
    ),
    http.post("http://tauri.local/get_current_omo_slim_provider_id", () =>
      HttpResponse.json(null),
    ),
  );
  setRuntimeSnapshot({
    status: "online",
    activeTargetId: "qa-host",
    generation: 77,
  });
  function RemotePage() {
    const query = useProvidersQuery("claude");
    const { switchProvider } = useProviderActions("claude", true);
    return (
      <>
        <output aria-label="remote-current">
          {query.data?.currentProviderId}
        </output>
        <ProviderList
          providers={query.data?.providers ?? {}}
          currentProviderId={query.data?.currentProviderId ?? ""}
          appId="claude"
          isLoading={query.isLoading}
          onSwitch={(provider) => void switchProvider(provider)}
          onEdit={vi.fn()}
          onDelete={vi.fn()}
          onDuplicate={vi.fn()}
          onOpenWebsite={vi.fn()}
          onExchange={exchange}
        />
      </>
    );
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <RemotePage />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  const run = await screen.findByRole("button", {
    name: "providerCard.action.switch",
  });
  expect(run.querySelector(".lucide-play")).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: /providerCard.section.added/ }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "交换 Remote B 到…" }));
  expect(exchange).toHaveBeenCalledWith(providers.b);
  expect(switched).not.toHaveBeenCalled();
  await user.click(run);
  await waitFor(() =>
    expect(screen.getByLabelText("remote-current")).toHaveTextContent("b"),
  );
  expect(switched).toHaveBeenCalledTimes(1);
  expect(switched).toHaveBeenCalledWith({
    command: "provider.switch",
    args: { app: "claude", id: "b", provider: providers.b },
    generation: 77,
  });
  expect(localSwitch).not.toHaveBeenCalled();
  expect(screen.getAllByText("providerCard.status.inUse")).toHaveLength(1);
  expect(document.querySelector(".lucide-minus")).not.toBeInTheDocument();
});
