import { useState } from "react";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { http, HttpResponse } from "msw";
import type { Provider } from "@/types";
import { ProviderSyncDialog } from "@/components/providers/ProviderSyncDialog";
import { server } from "../msw/server";
import { addProvider, getProviders, resetProviderState } from "../msw/state";

const TAURI_ENDPOINT = "http://tauri.local";

// —— 运行目标上下文：受控 mock ——
const runtimeMock = vi.hoisted(() => ({
  snapshot: {
    status: "online" as "online" | "offline",
    generation: 7,
    activeTargetId: "server-a" as string | null,
  },
  targets: [{ id: "server-a", name: "my-server", hostAlias: "prod" }],
}));

vi.mock("@/contexts/RuntimeTargetContext", () => ({
  useRuntimeTarget: () => runtimeMock,
}));

// —— toast mock ——
const toastMock = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast: toastMock }));

function createProvider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: overrides.id ?? "provider-1",
    name: overrides.name ?? "Test Provider",
    settingsConfig: overrides.settingsConfig ?? {},
    websiteUrl: overrides.websiteUrl,
    meta: overrides.meta,
    category: overrides.category,
    createdAt: overrides.createdAt,
  };
}

/**
 * 模拟远程后端：独立的内存 DB（真实后端是另一台机器，与本地 DB 不共享）。
 * 同时把 runtime snapshot 覆盖为 online，使 listRemote/addToRemote 走远程转发。
 */
function installRemoteBackend(
  seed: Record<string, Provider>,
): Record<string, Provider> {
  const remote: Record<string, Provider> = { ...seed };
  server.use(
    http.post(`${TAURI_ENDPOINT}/remote_get_runtime_snapshot`, () =>
      HttpResponse.json({
        status: "online",
        generation: 7,
        activeTargetId: "server-a",
      }),
    ),
    http.post(`${TAURI_ENDPOINT}/remote_invoke`, async ({ request }) => {
      const { command, args = {} } = (await request.json()) as {
        command: string;
        args?: Record<string, any>;
      };
      if (command === "provider.list") {
        return HttpResponse.json(remote);
      }
      if (command === "provider.add") {
        const p = args.provider as Provider;
        remote[p.id] = p;
        return HttpResponse.json(true);
      }
      return HttpResponse.json(
        { code: "COMMAND_NOT_EXPOSED", message: command },
        { status: 400 },
      );
    }),
  );
  return remote;
}

function renderDialog() {
  const onOpenChange = vi.fn();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const ui = (
    <QueryClientProvider client={queryClient}>
      <ProviderSyncDialog open onOpenChange={onOpenChange} appId="claude" />
    </QueryClientProvider>
  );
  render(ui);
  return { onOpenChange };
}

/** 可重新打开弹窗的宿主，模拟「关闭后再打开」的完整生命周期 */
function DialogHarness() {
  const [open, setOpen] = useState(false);
  const [queryClient] = useState(
    () => new QueryClient({ defaultOptions: { queries: { retry: false } } }),
  );
  return (
    <QueryClientProvider client={queryClient}>
      <button type="button" onClick={() => setOpen(true)}>
        open-dialog
      </button>
      <ProviderSyncDialog open={open} onOpenChange={setOpen} appId="claude" />
    </QueryClientProvider>
  );
}

const openDialog = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByRole("button", { name: "open-dialog" }));
  await waitFor(() =>
    expect(
      screen.getByText(/将.*的 Provider 同步到.*环境/),
    ).toBeInTheDocument(),
  );
};

const selectAndSync = async (
  user: ReturnType<typeof userEvent.setup>,
  name: string,
) => {
  await user.click(await screen.findByText(name));
  await user.click(screen.getByRole("button", { name: "同步到远程" }));
};

describe("ProviderSyncDialog", () => {
  beforeEach(() => {
    resetProviderState();
    toastMock.success.mockReset();
    toastMock.error.mockReset();
    runtimeMock.snapshot = {
      status: "online",
      generation: 7,
      activeTargetId: "server-a",
    };
    runtimeMock.targets = [
      { id: "server-a", name: "my-server", hostAlias: "prod" },
    ];
  });

  it("在线默认展示「远程 → 本机」，可切换方向后同步本机 Provider 到远程", async () => {
    addProvider(
      "claude",
      createProvider({
        id: "local-1",
        name: "Anthropic",
        websiteUrl: "https://anthropic.com",
      }),
    );
    const remote = installRemoteBackend({});
    const user = userEvent.setup();
    renderDialog();

    // 在线默认方向 = 远程 → 本机；远程库为空 → 空列表
    expect(
      await screen.findByText("将远程的 Provider 同步到本机环境"),
    ).toBeInTheDocument();
    expect(await screen.findByText("已选择 0 / 0")).toBeInTheDocument();

    // 切换为 本机 → 远程
    await user.click(screen.getByRole("button", { name: "切换方向" }));
    expect(
      screen.getByText("将本机的 Provider 同步到远程环境"),
    ).toBeInTheDocument();
    // 默认本地种子含 Claude Default / Claude Custom + 上面新增的 Anthropic
    expect(await screen.findByText("已选择 0 / 3")).toBeInTheDocument();

    await user.click(await screen.findByText("Anthropic"));
    expect(screen.getByText("1 个 Provider 将同步到远程")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "同步到远程" }));
    await waitFor(() => expect(toastMock.success).toHaveBeenCalled());
    const added = Object.values(remote);
    expect(added).toHaveLength(1);
    expect(added[0].name).toBe("Anthropic (local)");
    expect(added[0].id).not.toBe("local-1");
    // 同步副本不应携带 remote_synced 标记，否则会被目标端 list() 当作影子记录隐藏
    expect(
      (added[0].meta as Record<string, unknown> | undefined)?.remote_synced,
    ).toBeUndefined();
  });

  it("远程 → 本机：副本名称使用远程主机别名作后缀，写入本机", async () => {
    addProvider("claude", createProvider({ id: "local-1", name: "default" }));
    const remote = installRemoteBackend({
      "r-1": createProvider({ id: "r-1", name: "TeamRouter" }),
    });
    const user = userEvent.setup();
    renderDialog();

    expect(
      await screen.findByText("将远程的 Provider 同步到本机环境"),
    ).toBeInTheDocument();

    await user.click(screen.getByText("TeamRouter"));
    expect(screen.getByText("1 个 Provider 将同步到本机")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "同步到本机" }));
    await waitFor(() => expect(toastMock.success).toHaveBeenCalled());

    const local = getProviders("claude");
    const copied = Object.values(local).find((p) =>
      p.name.includes("TeamRouter"),
    );
    expect(copied?.name).toBe("TeamRouter (prod)");
    expect(copied?.id).not.toBe("r-1");
    expect(remote["r-1"]).toBeDefined();
  });

  it("目标端已有同源副本时，行内提示已同步过且不可勾选", async () => {
    addProvider("claude", createProvider({ id: "local-1", name: "Anthropic" }));
    installRemoteBackend({
      "r-1": createProvider({
        id: "r-1",
        name: "Anthropic (local)",
        meta: { remote_synced: false } as any,
      }),
    });
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByRole("button", { name: "切换方向" }));

    expect(await screen.findAllByText(/已同步过，将跳过/)).toHaveLength(2);
    // 该行 checkbox 被禁用，点击该行不产生选中
    const checkboxes = screen
      .getAllByRole("checkbox")
      .filter((c) => (c as HTMLInputElement).disabled);
    expect(checkboxes).toHaveLength(1);
    expect(screen.getByText("请选择要同步的 Provider")).toBeInTheDocument();
  });

  it("再次同步同一 Provider 不会产生重复：目标已有同名副本 → 跳过", async () => {
    addProvider("claude", createProvider({ id: "local-1", name: "Anthropic" }));
    const remote = installRemoteBackend({});
    const user = userEvent.setup();
    render(<DialogHarness />);

    // 第一次：同步到远程
    await openDialog(user);
    await user.click(screen.getByRole("button", { name: "切换方向" }));
    await selectAndSync(user, "Anthropic");
    await waitFor(() => expect(Object.values(remote)).toHaveLength(1));
    expect(Object.values(remote)[0].name).toBe("Anthropic (local)");

    // 重新打开：direction 保留「本机 → 远程」，远程已有「Anthropic (local)」→ 自动跳过
    await openDialog(user);
    expect(await screen.findAllByText(/已同步过，将跳过/)).toHaveLength(2);
    expect(screen.getByText("请选择要同步的 Provider")).toBeInTheDocument();

    // 再次同步 → 无新条目产生
    await user.click(screen.getByRole("button", { name: "同步到远程" }));
    await waitFor(() => expect(Object.values(remote)).toHaveLength(1));
  });

  it("往返同步不会把副本再复制回去：本地已带来源后缀的 Provider 被跳过", async () => {
    // 本地有：基础 Provider + 一个来自远程(prod)的副本
    addProvider("claude", createProvider({ id: "local-1", name: "Anthropic" }));
    addProvider(
      "claude",
      createProvider({ id: "local-2", name: "Anthropic (prod)" }),
    );
    const remote = installRemoteBackend({});
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByRole("button", { name: "切换方向" }));

    expect(await screen.findByText(/来自远程，将跳过/)).toBeInTheDocument();

    await user.click(screen.getByText("Anthropic"));
    await user.click(screen.getByRole("button", { name: "同步到远程" }));
    await waitFor(() => expect(toastMock.success).toHaveBeenCalled());
    // 只有基础 Provider 被同步，回程副本不被复制
    expect(Object.values(remote)).toHaveLength(1);
    expect(Object.values(remote)[0].name).toBe("Anthropic (local)");
  });

  it("离线时锁定为本机 → 远程，方向切换禁用，同步远程失败有错误提示", async () => {
    addProvider("claude", createProvider({ id: "local-1", name: "Anthropic" }));
    runtimeMock.snapshot = {
      status: "offline",
      generation: 0,
      activeTargetId: null,
    };
    const user = userEvent.setup();
    renderDialog();

    expect(
      await screen.findByText("将本机的 Provider 同步到远程环境"),
    ).toBeInTheDocument();
    // 离线时方向按钮禁用（锁图标）
    const swap = screen.getByRole("button", { name: "切换方向" });
    expect(swap).toBeDisabled();

    await user.click(screen.getByText("Anthropic"));
    await user.click(screen.getByRole("button", { name: "同步到远程" }));
    await waitFor(() => expect(toastMock.error).toHaveBeenCalled());
  });

  it("搜索按名称过滤列表，无匹配显示空态", async () => {
    addProvider("claude", createProvider({ id: "a", name: "Anthropic" }));
    addProvider("claude", createProvider({ id: "b", name: "OpenRouter" }));
    installRemoteBackend({});
    const user = userEvent.setup();
    renderDialog();
    await user.click(
      screen.getByRole("button", { name: "切换方向" }) as HTMLElement,
    );

    expect(await screen.findByText("Anthropic")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText(/搜索/), {
      target: { value: "openrouter" },
    });
    expect(screen.queryByText("Anthropic")).not.toBeInTheDocument();
    expect(screen.getByText("OpenRouter")).toBeInTheDocument();

    fireEvent.change(screen.getByPlaceholderText(/搜索/), {
      target: { value: "zzz" },
    });
    expect(await screen.findByText("没有匹配的 Provider")).toBeInTheDocument();
  });
});
