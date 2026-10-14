import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ProviderExchangeDialog,
  type ProviderExchangeSource,
} from "@/components/providers/ProviderExchangeDialog";
import { ProviderCardActions } from "@/components/providers/ProviderCardActions";
import type { Provider } from "@/types";

const api = vi.hoisted(() => ({
  listExchangeTarget: vi.fn(),
  addExchangeCopy: vi.fn(),
}));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("@/lib/api/providers", () => ({ providersApi: api }));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/contexts/RuntimeTargetContext", () => ({
  useRuntimeTarget: () => ({
    targets: [
      { id: "remote-a", name: "Server A", hostAlias: "a" },
      { id: "remote-b", name: "Server B", hostAlias: "b" },
    ],
    snapshot: { status: "online", generation: 7, activeTargetId: "remote-a" },
  }),
}));
const provider: Provider = {
  id: "source",
  name: "Vendor",
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: "https://api.example.com/v1",
      ANTHROPIC_AUTH_TOKEN: "source-secret",
      ANTHROPIC_MODEL: "vendor-model",
    },
  },
};

function mount(source: ProviderExchangeSource = { provider, appId: "claude" }) {
  const onOpenChange = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const ui = (value: ProviderExchangeSource | null) => (
    <QueryClientProvider client={client}>
      <ProviderExchangeDialog source={value} onOpenChange={onOpenChange} />
    </QueryClientProvider>
  );
  const view = render(ui(source));
  return {
    onOpenChange,
    rerender: (value: ProviderExchangeSource | null) =>
      view.rerender(ui(value)),
  };
}
const confirm = () => screen.getByRole("button", { name: /创建副本到/ });
const selectApp = (value: string) =>
  fireEvent.change(screen.getByLabelText("目标应用"), { target: { value } });
const selectTarget = (value: string) =>
  fireEvent.change(screen.getByLabelText("目标环境"), { target: { value } });

describe("ProviderExchangeDialog", () => {
  beforeEach(() => {
    api.listExchangeTarget.mockResolvedValue({});
    api.addExchangeCopy.mockResolvedValue(true);
  });

  it("每个供应商行内有独立交换按钮，点击只触发该行回调", async () => {
    const exchange = vi.fn();
    const edit = vi.fn();
    render(
      <ProviderCardActions
        providerName="Vendor"
        presentation={{ buttons: [], chips: [] }}
        onEdit={edit}
        onDelete={vi.fn()}
        onExchange={exchange}
      />,
    );
    await userEvent.click(
      screen.getByRole("button", { name: "交换 Vendor 到…" }),
    );
    expect(exchange).toHaveBeenCalledTimes(1);
    expect(edit).not.toHaveBeenCalled();
  });
  it("可复制到另一应用和另一台远程机器，源供应商保留", async () => {
    const before = structuredClone(provider);
    const { onOpenChange } = mount();
    selectApp("codex");
    selectTarget("remote-b");
    await waitFor(() => expect(confirm()).toBeEnabled());
    expect(api.addExchangeCopy).not.toHaveBeenCalled();
    await userEvent.click(confirm());
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(api.addExchangeCopy).toHaveBeenCalledWith(
      expect.objectContaining({
        id: expect.any(String),
        name: "Vendor",
        settingsConfig: expect.objectContaining({
          auth: { OPENAI_API_KEY: "source-secret" },
        }),
      }),
      "codex",
      "remote-b",
    );
    expect(api.addExchangeCopy.mock.calls[0][0].id).not.toBe(provider.id);
    expect(provider).toEqual(before);
    expect(toast.success).toHaveBeenCalledOnce();
  });
  it("远程来源也能复制到本机的另一应用", async () => {
    mount({ provider, appId: "claude", targetId: "remote-a" });
    selectTarget("");
    selectApp("gemini");
    await waitFor(() => expect(confirm()).toBeEnabled());
    expect(screen.getByLabelText("上游协议")).toHaveValue("gemini_native");
    await userEvent.click(confirm());
    await waitFor(() =>
      expect(api.addExchangeCopy).toHaveBeenCalledWith(
        expect.objectContaining({
          settingsConfig: {
            env: {
              GOOGLE_GEMINI_BASE_URL: "https://api.example.com/v1",
              GEMINI_API_KEY: "source-secret",
              GEMINI_MODEL: "vendor-model",
            },
          },
        }),
        "gemini",
        undefined,
      ),
    );
  });
  it("同名建议副本名称，手工冲突仍阻止提交", async () => {
    api.listExchangeTarget.mockResolvedValue({ source: provider });
    mount();
    await waitFor(() =>
      expect(screen.getByLabelText("副本名称")).toHaveValue("Vendor (副本)"),
    );
    fireEvent.change(screen.getByLabelText("副本名称"), {
      target: { value: "Vendor" },
    });
    expect(confirm()).toBeDisabled();
    expect(
      screen.getByText("目标已有同名供应商，请修改副本名称。"),
    ).toBeInTheDocument();
  });
  it("提交前重新检查冲突，不覆盖弹窗期间新增的同名供应商", async () => {
    mount();
    await waitFor(() => expect(confirm()).toBeEnabled());
    api.listExchangeTarget.mockResolvedValue({ other: provider });
    await userEvent.click(confirm());
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("目标已有同名供应商"),
    );
    expect(api.addExchangeCopy).not.toHaveBeenCalled();
  });
  it("目标读取失败禁止创建，重试成功才恢复", async () => {
    api.listExchangeTarget.mockRejectedValueOnce(new Error("Target offline"));
    mount();
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("Target offline"),
    );
    expect(confirm()).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "重新读取" }));
    await waitFor(() => expect(confirm()).toBeEnabled());
  });
  it("迟到的旧目标列表不能覆盖新选择", async () => {
    let release: (value: Record<string, Provider>) => void = () => undefined;
    api.listExchangeTarget.mockImplementation((_app, target) =>
      target === "remote-a"
        ? new Promise((resolve) => {
            release = resolve;
          })
        : Promise.resolve({}),
    );
    mount();
    selectTarget("remote-a");
    selectTarget("remote-b");
    await waitFor(() => expect(confirm()).toBeEnabled());
    release({ late: provider });
    await waitFor(() =>
      expect(screen.getByLabelText("副本名称")).toHaveValue("Vendor"),
    );
    expect(screen.getByLabelText("目标环境")).toHaveValue("remote-b");
  });
  it("保存失败保留弹窗，返回 false 不显示成功", async () => {
    api.addExchangeCopy.mockResolvedValue(false);
    const { onOpenChange } = mount();
    await waitFor(() => expect(confirm()).toBeEnabled());
    await userEvent.click(confirm());
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("交换失败"),
    );
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });
  it("跨机器账号引用需要手工目标连接信息", async () => {
    mount({
      provider: {
        id: "managed",
        name: "Managed",
        settingsConfig: {},
        meta: {
          providerType: "codex_oauth",
          authBinding: { source: "managed_account", accountId: "a" },
        },
      },
      appId: "codex",
      targetId: "remote-a",
    });
    selectTarget("remote-b");
    await waitFor(() =>
      expect(
        screen.getByText(/账号登录不能跨应用或机器复制/),
      ).toBeInTheDocument(),
    );
    expect(confirm()).toBeDisabled();
    fireEvent.change(screen.getByLabelText("目标接口地址"), {
      target: { value: "https://api.example.com/v1" },
    });
    fireEvent.change(screen.getByLabelText("目标模型"), {
      target: { value: "manual-model" },
    });
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "manual-key" },
    });
    await waitFor(() => expect(confirm()).toBeEnabled());
  });
  it("预览默认隐藏密钥，取消不写任何供应商", async () => {
    const { onOpenChange } = mount();
    expect(screen.getByLabelText("API Key")).toHaveAttribute(
      "type",
      "password",
    );
    await userEvent.click(screen.getByText("查看目标配置（密钥已隐藏）"));
    expect(document.querySelector("pre")).not.toHaveTextContent(
      "source-secret",
    );
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(api.addExchangeCopy).not.toHaveBeenCalled();
  });

  it("写入成功但响应丢失，重试确认原副本而不重复写入", async () => {
    api.addExchangeCopy.mockImplementationOnce(async (copy: Provider) => {
      api.listExchangeTarget.mockResolvedValue({ [copy.id]: copy });
      throw new Error("Response lost");
    });
    const { onOpenChange } = mount();
    await waitFor(() => expect(confirm()).toBeEnabled());
    await userEvent.click(confirm());
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("Response lost"),
    );
    await userEvent.click(confirm());
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(api.addExchangeCopy).toHaveBeenCalledOnce();
  });
  it("保存期间禁止重复提交、修改目标或关闭弹窗", async () => {
    let release: (saved: boolean) => void = () => undefined;
    api.addExchangeCopy.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    const { onOpenChange } = mount();
    await waitFor(() => expect(confirm()).toBeEnabled());
    await userEvent.click(confirm());
    await waitFor(() => expect(api.addExchangeCopy).toHaveBeenCalledOnce());
    expect(confirm()).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    expect(screen.getByLabelText("目标环境")).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onOpenChange).not.toHaveBeenCalled();
    release(true);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
  it("清空目标连接字段不能通过同应用校验", async () => {
    mount();
    await waitFor(() => expect(confirm()).toBeEnabled());
    fireEvent.change(screen.getByLabelText("目标接口地址"), {
      target: { value: "" },
    });
    expect(confirm()).toBeDisabled();
  });
  it("坏 TOML 给出修复源配置的提示，不静默创建空配置", async () => {
    mount({
      provider: {
        id: "bad",
        name: "Bad",
        settingsConfig: { config: 'model = "unfinished' },
      },
      appId: "codex",
    });
    await waitFor(() =>
      expect(screen.getByText(/源供应商配置无法解析/)).toBeInTheDocument(),
    );
    expect(confirm()).toBeDisabled();
  });
});
