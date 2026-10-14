import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { AppId } from "@/lib/api/types";
import type { Provider, ProviderMeta } from "@/types";
import { deepClone } from "@/utils/deepClone";
import { extractProviderBaseUrl } from "@/utils/providerConfigUtils";
import {
  parseGrokBuildConfig,
  updateGrokBuildConfig,
} from "@/utils/grokBuildConfig";
import { CLAUDE_DESKTOP_ROLE_ROUTE_IDS } from "@/config/claudeDesktopProviderPresets";

/** 交换只搬运连接信息；不同应用的配置容器与协议标记在这里集中转换。 */
export type ExchangeApiFormat = NonNullable<ProviderMeta["apiFormat"]>;
export interface ExchangeFields {
  baseUrl: string;
  apiKey: string;
  model: string;
  apiFormat: ExchangeApiFormat;
}
/** 对齐 Core validate_app：远端 Pi / MiniMax Code 尚无领域服务，不展示可写的假目标。 */
export function supportsRemoteExchange(app: AppId): boolean {
  return app !== "pi" && app !== "mcode";
}
const FORMATS: ExchangeApiFormat[] = [
  "anthropic",
  "openai_chat",
  "openai_responses",
  "gemini_native",
];
const API_NAMES: Record<ExchangeApiFormat, string> = {
  anthropic: "anthropic-messages",
  openai_chat: "openai-completions",
  openai_responses: "openai-responses",
  gemini_native: "google-generative-ai",
};
const asRecord = (value: unknown): Record<string, any> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, any>)
    : {};
const text = (...values: unknown[]): string =>
  (values.find(
    (value) => typeof value === "string" && value.trim(),
  ) as string) || "";

/** 单模型编辑只调整首选位置；同应用的其他模型与能力参数必须保留。 */
function copyModels(models: unknown, model: string, sameApp: boolean) {
  const existing = sameApp && Array.isArray(models) ? models : [];
  if (!model) return existing;
  return [
    existing.find((item) => item.id === model) ?? { id: model, name: model },
    ...existing.filter((item) => item.id !== model),
  ];
}

/** 应用只列出其实际支持的上游协议；改目标时不能把不兼容的协议标记直接沿用。 */
export function exchangeFormats(app: AppId): ExchangeApiFormat[] {
  if (app === "gemini") return ["gemini_native"];
  if (["codex", "grokbuild", "hermes", "mcode"].includes(app))
    return FORMATS.filter((format) => format !== "gemini_native");
  return FORMATS;
}
export function defaultExchangeFormat(app: AppId): ExchangeApiFormat {
  if (app === "gemini") return "gemini_native";
  if (app === "claude" || app === "claude-desktop") return "anthropic";
  if (app === "codex" || app === "grokbuild") return "openai_responses";
  return "openai_chat";
}

export function extractExchangeFields(
  provider: Provider,
  app: AppId,
): ExchangeFields {
  const config = provider.settingsConfig;
  const env = asRecord(config.env);
  const options = asRecord(config.options);
  let toml: Record<string, any> = {};
  // 不靠正则猜 TOML，保证命名表、转义密钥和内联表的含义一致。
  if (app === "codex" && typeof config.config === "string")
    toml = asRecord(parseToml(config.config));
  const selected = asRecord(
    asRecord(toml.model_providers)[toml.model_provider],
  );
  const models = Array.isArray(config.models)
    ? config.models
    : Object.keys(asRecord(config.models)).map((id) => ({ id }));
  const desktopModel = Object.values(
    provider.meta?.claudeDesktopModelRoutes ?? {},
  )[0]?.model;
  const grok =
    app === "grokbuild"
      ? parseGrokBuildConfig(config.config, provider.name)
      : undefined;
  let apiFormat = provider.meta?.apiFormat ?? defaultExchangeFormat(app);
  const protocol = text(config.api, config.api_mode);
  if (protocol.includes("anthropic") || config.npm === "@ai-sdk/anthropic")
    apiFormat = "anthropic";
  else if (protocol.includes("responses") || config.npm === "@ai-sdk/openai")
    apiFormat = "openai_responses";
  else if (protocol.includes("google") || config.npm === "@ai-sdk/google")
    apiFormat = "gemini_native";
  else if (
    protocol.includes("completions") ||
    config.npm === "@ai-sdk/openai-compatible"
  )
    apiFormat = "openai_chat";
  if (grok && !provider.meta?.apiFormat)
    apiFormat = grok.apiBackend === "chat" ? "openai_chat" : "openai_responses";
  return {
    baseUrl: text(
      grok?.baseUrl,
      extractProviderBaseUrl(config),
      env.OPENAI_BASE_URL,
    ),
    apiKey: text(
      grok?.apiKey,
      config.auth?.OPENAI_API_KEY,
      selected.experimental_bearer_token,
      env.ANTHROPIC_AUTH_TOKEN,
      env.ANTHROPIC_API_KEY,
      env.GEMINI_API_KEY,
      env.CODEX_API_KEY,
      config.apiKey,
      config.api_key,
      options.apiKey,
    ),
    model: text(
      grok?.upstreamModel,
      toml.model,
      env.ANTHROPIC_MODEL,
      env.ANTHROPIC_DEFAULT_SONNET_MODEL,
      env.GEMINI_MODEL,
      desktopModel,
      models[0]?.id,
      models[0]?.model,
      config.model,
    ),
    apiFormat,
  };
}

/** 同名只创建新名字，不覆盖；比较规则与弹窗冲突校验保持一致。 */
export function suggestExchangeName(
  name: string,
  names: Set<string>,
  copyLabel: string,
): string {
  if (!names.has(name.trim())) return name;
  const base = `${name} (${copyLabel})`;
  let candidate = base;
  for (let i = 2; names.has(candidate); i += 1) candidate = `${base} ${i}`;
  return candidate;
}

function configureCopy(
  config: Record<string, any>,
  app: AppId,
  fields: ExchangeFields,
  name: string,
  sameApp: boolean,
) {
  const { baseUrl, apiKey, model, apiFormat } = fields;
  const env = asRecord(config.env);
  switch (app) {
    case "claude":
    case "claude-desktop": {
      const keyField =
        "ANTHROPIC_API_KEY" in env && !("ANTHROPIC_AUTH_TOKEN" in env)
          ? "ANTHROPIC_API_KEY"
          : "ANTHROPIC_AUTH_TOKEN";
      config.env = { ...env, ANTHROPIC_BASE_URL: baseUrl, [keyField]: apiKey };
      if (app === "claude" && model) {
        config.env.ANTHROPIC_MODEL = model;
        // 跨应用新增时四档使用选定的上游模型；同应用保留已有的分档映射。
        if (!sameApp)
          for (const role of ["SONNET", "OPUS", "HAIKU"])
            config.env[`ANTHROPIC_DEFAULT_${role}_MODEL`] = model;
      }
      break;
    }
    case "codex": {
      const root =
        typeof config.config === "string"
          ? asRecord(parseToml(config.config))
          : {};
      // openai 是官方登录专用标识，手工 API 副本必须放在可配置的 custom 表中。
      const providerKey =
        text(root.model_provider) === "openai"
          ? "custom"
          : text(root.model_provider) || "custom";
      const providers = asRecord(root.model_providers);
      const selected = asRecord(providers[providerKey]);
      root.model_provider = providerKey;
      if (model) root.model = model;
      root.model_providers = {
        ...providers,
        [providerKey]: {
          ...selected,
          name: text(selected.name) || name,
          base_url: baseUrl,
          wire_api: "responses",
          requires_openai_auth: true,
        },
      };
      // 旧配置若用内联 bearer，它优先于 auth.json；必须同步更新，不能留下原来的密钥。
      if ("experimental_bearer_token" in selected)
        root.model_providers[providerKey].experimental_bearer_token = apiKey;
      config.config = stringifyToml(root);
      config.auth = { ...asRecord(config.auth), OPENAI_API_KEY: apiKey };
      break;
    }
    case "gemini":
      config.env = {
        ...env,
        GOOGLE_GEMINI_BASE_URL: baseUrl,
        GEMINI_API_KEY: apiKey,
        ...(model ? { GEMINI_MODEL: model } : {}),
      };
      break;
    case "grokbuild":
      config.config = updateGrokBuildConfig(config.config, {
        model: model || "grok-4.5",
        upstreamModel: model,
        baseUrl,
        apiKey,
        name,
        // Grok Build 客户端固定发 Responses，其他上游协议由 meta.apiFormat 指导本地路由转换。
        apiBackend: "responses",
        contextWindow: parseGrokBuildConfig(config.config, name).contextWindow,
      });
      break;
    case "opencode":
    case "mcode":
      config.name = name;
      config.options = {
        ...asRecord(config.options),
        baseURL: baseUrl,
        apiKey,
      };
      if (app === "opencode")
        config.npm = {
          anthropic: "@ai-sdk/anthropic",
          openai_chat: "@ai-sdk/openai-compatible",
          openai_responses: "@ai-sdk/openai",
          gemini_native: "@ai-sdk/google",
        }[apiFormat];
      else {
        config.api = API_NAMES[apiFormat];
        config.kind = "custom";
        config.enabled = false;
      }
      config.models = {
        ...asRecord(config.models),
        ...(model
          ? { [model]: asRecord(config.models)[model] ?? { name: model } }
          : {}),
      };
      break;
    case "openclaw":
    case "pi":
      config.baseUrl = baseUrl;
      config.apiKey = apiKey;
      config.api = API_NAMES[apiFormat];
      if (app === "pi") config.name = name;
      config.models = copyModels(config.models, model, sameApp);
      break;
    case "hermes":
      config.name = name;
      config.base_url = baseUrl;
      config.api_key = apiKey;
      config.api_mode = {
        anthropic: "anthropic_messages",
        openai_chat: "chat_completions",
        openai_responses: "codex_responses",
        gemini_native: "chat_completions",
      }[apiFormat];
      delete config._cc_source;
      config.models = copyModels(config.models, model, sameApp);
      break;
  }
  return config;
}

/**
 * 独立副本使用新 ID，清除排序、故障队列和远程影子状态。
 * 同应用保留完整设置；跨应用只转连接字段，不能把源应用的 live/账号身份带到目标应用。
 * 账号与本机资源无法跨机器复用，跨机器副本必须使用用户提供的普通 API 认证。
 */
export function buildExchangeCopy(
  source: Provider,
  sourceApp: AppId,
  targetApp: AppId,
  fields: ExchangeFields,
  name: string,
  sameHost: boolean,
  id: string,
): Provider {
  const sameApp = sourceApp === targetApp;
  const original = extractExchangeFields(source, sourceApp);
  const unchanged = Object.keys(original).every(
    (key) =>
      original[key as keyof ExchangeFields] ===
      fields[key as keyof ExchangeFields],
  );
  const meta = (sameApp ? deepClone(source.meta ?? {}) : {}) as ProviderMeta &
    Record<string, unknown>;
  if (!sameApp) {
    for (const key of [
      "usage_script",
      "custom_endpoints",
      "customUserAgent",
      "isFullUrl",
    ] as const) {
      if (source.meta?.[key] !== undefined)
        (meta as Record<string, unknown>)[key] = deepClone(source.meta[key]);
    }
  }
  delete meta.remoteSynced;
  delete meta.remote_synced;
  delete meta.liveConfigManaged;
  const keepAccount = sameApp && sameHost && unchanged;
  if (!keepAccount) {
    delete meta.authBinding;
    delete meta.providerType;
    delete meta.githubAccountId;
    delete meta.codexCopilotApiFormat;
    delete meta.codexFastMode;
    // OAuth token 属于源主机的登录身份，手工 API 副本只保留用户填写的 API Key。
  }
  // 名称改变不重写未改动的同应用配置，以保留多模型目录、特殊协议与自定义参数。
  const settingsConfig =
    sameApp && unchanged
      ? deepClone(source.settingsConfig)
      : configureCopy(
          sameApp ? deepClone(source.settingsConfig) : {},
          targetApp,
          fields,
          name,
          sameApp,
        );
  if (!keepAccount && targetApp === "codex")
    settingsConfig.auth = { OPENAI_API_KEY: fields.apiKey };
  // 副本不再属于 Hermes Web UI 的只读源，也不是 MCode 已启用的 live 项。
  if (targetApp === "hermes") delete settingsConfig._cc_source;
  if (targetApp === "mcode") settingsConfig.enabled = false;
  if (
    sameApp &&
    unchanged &&
    !sameHost &&
    (source.meta?.authBinding?.source === "managed_account" ||
      source.meta?.providerType ||
      source.meta?.githubAccountId)
  ) {
    throw new Error("provider.exchange.accountRequired");
  }
  if (!sameApp || !unchanged) meta.apiFormat = fields.apiFormat;
  // 非原生协议必须走 Desktop 代理，不能把任意上游模型伪装成 Desktop 的直连模型。
  if (
    targetApp === "claude-desktop" &&
    (!sameApp ||
      fields.model !== original.model ||
      fields.apiFormat !== original.apiFormat)
  ) {
    meta.claudeDesktopMode = "proxy";
    meta.claudeDesktopModelRoutes = Object.fromEntries(
      Object.values(CLAUDE_DESKTOP_ROLE_ROUTE_IDS).map((route) => [
        route,
        { model: fields.model },
      ]),
    );
  }
  return {
    id,
    name: name.trim(),
    settingsConfig,
    meta,
    websiteUrl: source.websiteUrl,
    notes: source.notes,
    icon: source.icon,
    iconColor: source.iconColor,
    category:
      keepAccount || (sameApp && source.category !== "official")
        ? source.category
        : "custom",
    createdAt: Date.now(),
    inFailoverQueue: false,
  };
}

/** 预览永远遮住密钥和令牌，包括 TOML/字符串内的字段；不能把明文凭据复制到提示或日志。 */
export function exchangePreview(provider: Provider): string {
  const redact = (value: unknown, key = ""): unknown => {
    // 用量脚本的字符串可能内嵌独立凭据，预览省略脚本正文，不影响副本实际保存。
    if (key === "code" && typeof value === "string")
      return value ? "••••••••" : "";
    if (
      /key|token|password|secret|authorization/i.test(key) &&
      typeof value === "string"
    )
      return value ? "••••••••" : "";
    if (Array.isArray(value)) return value.map((item) => redact(item));
    if (value && typeof value === "object")
      return Object.fromEntries(
        Object.entries(value).map(([name, item]) => [name, redact(item, name)]),
      );
    if (key === "config" && typeof value === "string") {
      try {
        return stringifyToml(redact(parseToml(value)) as Record<string, any>);
      } catch {
        return "";
      }
    }
    return value;
  };
  return JSON.stringify(
    redact({ settingsConfig: provider.settingsConfig, meta: provider.meta }),
    null,
    2,
  );
}
