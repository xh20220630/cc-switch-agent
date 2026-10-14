import { describe, expect, it } from "vitest";
import { parse as parseToml } from "smol-toml";
import { APP_IDS } from "@/config/appConfig";
import {
  buildExchangeCopy,
  defaultExchangeFormat,
  exchangeFormats,
  exchangePreview,
  extractExchangeFields,
  suggestExchangeName,
} from "@/utils/providerExchange";
import type { Provider } from "@/types";

const source: Provider = {
  id: "source",
  name: "Vendor",
  category: "third_party",
  sortIndex: 1,
  inFailoverQueue: true,
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: "https://api.example.com/v1",
      ANTHROPIC_AUTH_TOKEN: "source-secret",
      ANTHROPIC_MODEL: "vendor-model",
    },
    custom: { preserve: true },
  },
  meta: {
    apiFormat: "anthropic",
    stackModels: [{ model: "vendor-model" }],
    liveConfigManaged: true,
  },
};

describe("供应商交换配置", () => {
  it.each(APP_IDS)("跨应用创建 %s 副本，连接字段能按目标格式读回", (app) => {
    const fields = {
      ...extractExchangeFields(source, "claude"),
      apiFormat: defaultExchangeFormat(app),
    };
    const copy = buildExchangeCopy(
      source,
      "claude",
      app,
      fields,
      "Copy",
      false,
      "copy",
    );
    const readBack = extractExchangeFields(copy, app);
    expect(readBack).toEqual(fields);
    expect(copy.id).toBe("copy");
    expect(copy.inFailoverQueue).toBe(false);
    expect(copy.sortIndex).toBeUndefined();
    expect(copy.meta?.liveConfigManaged).toBeUndefined();
    expect(source.settingsConfig.custom.preserve).toBe(true);
    if (app !== "claude") {
      expect(copy.settingsConfig.custom).toBeUndefined();
      expect(copy.meta?.stackModels).toBeUndefined();
    }
  });
  it("同应用保持多模型与自定义配置，修改副本不会影响源", () => {
    const copy = buildExchangeCopy(
      source,
      "claude",
      "claude",
      extractExchangeFields(source, "claude"),
      "Copy",
      true,
      "copy",
    );
    expect(copy.settingsConfig).toEqual(source.settingsConfig);
    copy.settingsConfig.custom.preserve = false;
    expect(source.settingsConfig.custom.preserve).toBe(true);
    expect(copy.meta?.stackModels).toEqual(source.meta?.stackModels);
  });
  it("Codex 正确读取选定 TOML provider，更新内联 bearer 并保留其他 TOML 字段", () => {
    const codex: Provider = {
      id: "c",
      name: "Codex",
      settingsConfig: {
        auth: {},
        config:
          'model = "m"\nmodel_provider = "named"\nsandbox_mode = "read-only"\n[model_providers.named]\nbase_url = "https://api.example.com/v1"\nexperimental_bearer_token = "old-secret"\nwire_api = "responses"\n',
      },
    };
    const fields = extractExchangeFields(codex, "codex");
    expect(fields.apiKey).toBe("old-secret");
    const copy = buildExchangeCopy(
      codex,
      "codex",
      "codex",
      { ...fields, apiKey: 'new-"secret' },
      "Copy",
      true,
      "new",
    );
    const root = parseToml(copy.settingsConfig.config) as any;
    expect(root.sandbox_mode).toBe("read-only");
    expect(root.model_providers.named.experimental_bearer_token).toBe(
      'new-"secret',
    );
    expect(copy.settingsConfig.auth.OPENAI_API_KEY).toBe('new-"secret');
  });
  it("跨机器拒绝复制账号引用，手工 API 副本去掉账号和 OAuth token", () => {
    const managed: Provider = {
      id: "m",
      name: "Managed",
      settingsConfig: {
        auth: { tokens: { access_token: "oauth-secret" } },
        config: 'model = "m"',
      },
      meta: {
        providerType: "codex_oauth",
        authBinding: { source: "managed_account", accountId: "source-account" },
      },
    };
    const original = extractExchangeFields(managed, "codex");
    expect(() =>
      buildExchangeCopy(
        managed,
        "codex",
        "codex",
        original,
        "Copy",
        false,
        "new",
      ),
    ).toThrow("provider.exchange.accountRequired");
    const copy = buildExchangeCopy(
      managed,
      "codex",
      "codex",
      {
        ...original,
        apiKey: "manual-secret",
        baseUrl: "https://api.example.com/v1",
      },
      "Copy",
      false,
      "new",
    );
    expect(copy.meta?.authBinding).toBeUndefined();
    expect(copy.meta?.providerType).toBeUndefined();
    expect(copy.settingsConfig.auth).toEqual({
      OPENAI_API_KEY: "manual-secret",
    });
  });
  it("预览隐藏对象、TOML、请求头及用量脚本中的密钥", () => {
    const copy = buildExchangeCopy(
      source,
      "claude",
      "codex",
      { ...extractExchangeFields(source, "claude"), apiKey: "target-secret" },
      "Copy",
      false,
      "new",
    );
    copy.settingsConfig.config += 'experimental_bearer_token = "toml-secret"\n';
    copy.settingsConfig.headers = { Authorization: "Bearer header-secret" };
    copy.meta!.usage_script = {
      enabled: true,
      language: "javascript",
      code: '{"apiKey":"script-secret"}',
    };
    const preview = exchangePreview(copy);
    for (const secret of [
      "target-secret",
      "toml-secret",
      "header-secret",
      "script-secret",
    ])
      expect(preview).not.toContain(secret);
    expect(preview).toContain("••••••••");
  });
  it("同名副本不覆盖已有名字", () => {
    expect(
      suggestExchangeName(
        "Vendor",
        new Set(["Vendor", "Vendor (副本)"]),
        "副本",
      ),
    ).toBe("Vendor (副本) 2");
  });
  it("目标只开放支持的协议", () => {
    expect(exchangeFormats("gemini")).toEqual(["gemini_native"]);
    expect(exchangeFormats("codex")).not.toContain("gemini_native");
    expect(exchangeFormats("claude")).toContain("openai_responses");
    expect(exchangeFormats("grokbuild")).toContain("anthropic");
  });
  it("Grok Build 保持客户端 Responses 后端，同时保留实际的上游协议", () => {
    const copy = buildExchangeCopy(
      source,
      "claude",
      "grokbuild",
      extractExchangeFields(source, "claude"),
      "Copy",
      false,
      "new",
    );
    expect(extractExchangeFields(copy, "grokbuild").apiFormat).toBe(
      "anthropic",
    );
    const config = parseToml(copy.settingsConfig.config) as any;
    expect(config.model["vendor-model"].api_backend).toBe("responses");
  });
  it("Hermes 副本解除只读来源，调整首选模型保留其余能力参数", () => {
    const hermes: Provider = {
      id: "h",
      name: "Hermes",
      settingsConfig: {
        _cc_source: "providers_dict",
        base_url: "https://api.example.com/v1",
        api_key: "key",
        models: [
          { id: "a", context_length: 1234 },
          { id: "b", context_length: 5678 },
        ],
      },
    };
    const fields = extractExchangeFields(hermes, "hermes");
    const copy = buildExchangeCopy(
      hermes,
      "hermes",
      "hermes",
      { ...fields, model: "b" },
      "Copy",
      true,
      "new",
    );
    expect(copy.settingsConfig._cc_source).toBeUndefined();
    expect(copy.settingsConfig.models).toEqual([
      { id: "b", context_length: 5678 },
      { id: "a", context_length: 1234 },
    ]);
    expect(hermes.settingsConfig._cc_source).toBe("providers_dict");
  });
  it("同应用 Desktop 调整模型会更新目标路由映射", () => {
    const desktop = buildExchangeCopy(
      source,
      "claude",
      "claude-desktop",
      extractExchangeFields(source, "claude"),
      "Desktop",
      true,
      "d",
    );
    const fields = extractExchangeFields(desktop, "claude-desktop");
    const copy = buildExchangeCopy(
      desktop,
      "claude-desktop",
      "claude-desktop",
      { ...fields, model: "new-model" },
      "Copy",
      true,
      "new",
    );
    expect(extractExchangeFields(copy, "claude-desktop").model).toBe(
      "new-model",
    );
    expect(copy.meta?.claudeDesktopMode).toBe("proxy");
  });
});
