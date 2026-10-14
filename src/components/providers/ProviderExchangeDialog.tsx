import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowRight, ArrowLeftRight, Eye, EyeOff, Loader2 } from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Notice } from "@/components/ui/notice";
import { APP_IDS } from "@/config/appConfig";
import { APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import { useRuntimeTarget } from "@/contexts/RuntimeTargetContext";
import { providersApi } from "@/lib/api/providers";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  buildExchangeCopy,
  defaultExchangeFormat,
  exchangeFormats,
  exchangePreview,
  extractExchangeFields,
  suggestExchangeName,
  supportsRemoteExchange,
  type ExchangeApiFormat,
  type ExchangeFields,
} from "@/utils/providerExchange";
import type { AppId } from "@/lib/api/types";
import type { Provider } from "@/types";

/** 打开时固定源快照和主机身份，弹窗期间切换页面不能让副本来源跟着变化。 */
export interface ProviderExchangeSource {
  provider: Provider;
  appId: AppId;
  targetId?: string;
}
interface ProviderExchangeDialogProps {
  source: ProviderExchangeSource | null;
  onOpenChange: (open: boolean) => void;
}
const selectClass =
  "h-9 w-full rounded-control border border-border bg-surface px-3 text-body text-fg-1 focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-50";
const formatLabels: Record<ExchangeApiFormat, string> = {
  anthropic: "Anthropic Messages",
  openai_chat: "OpenAI Chat Completions",
  openai_responses: "OpenAI Responses",
  gemini_native: "Gemini Native",
};

/** 每行入口只操作一条供应商：选择位置 → 调整连接信息 → 创建待启用副本。 */
export function ProviderExchangeDialog({
  source,
  onOpenChange,
}: ProviderExchangeDialogProps) {
  const { t } = useTranslation();
  const { targets } = useRuntimeTarget();
  const queryClient = useQueryClient();
  const [preparedSource, setPreparedSource] =
    useState<ProviderExchangeSource | null>(null);
  const [targetId, setTargetId] = useState("");
  const [app, setApp] = useState<AppId>("claude");
  const [fields, setFields] = useState<ExchangeFields>({
    baseUrl: "",
    apiKey: "",
    model: "",
    apiFormat: "anthropic",
  });
  const [name, setName] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [names, setNames] = useState<Set<string>>(new Set());
  const [loadedScope, setLoadedScope] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const copyId = useRef("");
  const nameEdited = useRef(false);
  const submitting = useRef(false);
  const attemptedCopy = useRef<string | null>(null);
  const scope = `${targetId}:${app}`;
  const sameHost = (source?.targetId ?? "") === targetId;
  const labelFor = (id?: string) =>
    id
      ? (targets.find((target) => target.id === id)?.name ?? id)
      : t("provider.exchange.local", { defaultValue: "本机" });

  useEffect(() => {
    if (!source) return;
    copyId.current = crypto.randomUUID();
    attemptedCopy.current = null;
    nameEdited.current = false;
    setName(source.provider.name);
    setApp(source.appId);
    setTargetId(source.targetId ?? "");
    setShowKey(false);
    setError(null);
    setLoadedScope(null);
    setPreparedSource(source);
    try {
      setFields(extractExchangeFields(source.provider, source.appId));
    } catch {
      setFields({
        baseUrl: "",
        apiKey: "",
        model: "",
        apiFormat: defaultExchangeFormat(source.appId),
      });
    }
  }, [source]);

  useEffect(() => {
    // 先完成来源初始化再读取目标，避免远程行刚打开时按上一次弹窗的本机选择发请求。
    if (!source || preparedSource !== source) return;
    let cancelled = false;
    setLoading(true);
    setLoadedScope(null);
    setError(null);
    // 目标列表读取失败必须阻止创建，不能把读取错误当成空列表后静默写入。
    void providersApi
      .listExchangeTarget(app, targetId || undefined)
      .then((providers) => {
        if (cancelled) return;
        const nextNames = new Set(
          Object.values(providers).map((provider) => provider.name.trim()),
        );
        setNames(nextNames);
        if (!nameEdited.current)
          setName(
            suggestExchangeName(
              source.provider.name,
              nextNames,
              t("provider.exchange.copySuffix", { defaultValue: "副本" }),
            ),
          );
        setLoadedScope(scope);
      })
      .catch((cause) => {
        if (!cancelled) setError(extractErrorMessage(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [source, preparedSource, app, targetId, scope, reload, t]);

  const draft = useMemo(() => {
    if (!source) return null;
    try {
      return buildExchangeCopy(
        source.provider,
        source.appId,
        app,
        fields,
        name,
        sameHost,
        copyId.current,
      );
    } catch {
      return null;
    }
  }, [source, app, fields, name, sameHost]);
  const usesAccount =
    source?.provider.meta?.authBinding?.source === "managed_account" ||
    Boolean(source?.provider.meta?.providerType);
  const needsAccount = usesAccount && (!sameHost || app !== source?.appId);
  const nameConflict = names.has(name.trim());
  const invalidUrl =
    Boolean(fields.baseUrl.trim()) &&
    !/^https?:\/\/[^\s]+$/i.test(fields.baseUrl.trim());
  const canKeepOpaque = sameHost && app === source?.appId;
  // 只允许完整保留未修改的同应用配置；手工清空已填写字段不能借这个分支绕过校验。
  const sourceFields = useMemo(() => {
    if (!source) return null;
    try {
      return extractExchangeFields(source.provider, source.appId);
    } catch {
      return null;
    }
  }, [source]);
  const preserveUnchanged =
    canKeepOpaque &&
    sourceFields &&
    Object.keys(sourceFields).every(
      (key) =>
        sourceFields[key as keyof ExchangeFields] ===
        fields[key as keyof ExchangeFields],
    );
  const unsupportedTarget = Boolean(targetId) && !supportsRemoteExchange(app);
  const missingFields =
    !preserveUnchanged &&
    (!fields.baseUrl.trim() ||
      !fields.model.trim() ||
      (needsAccount && !fields.apiKey.trim()));
  const targetMissing =
    Boolean(targetId) && !targets.some((target) => target.id === targetId);
  const valid = Boolean(
    draft &&
      name.trim() &&
      !nameConflict &&
      !invalidUrl &&
      !missingFields &&
      !targetMissing &&
      !unsupportedTarget &&
      loadedScope === scope &&
      !loading,
  );
  const preview = draft ? exchangePreview(draft) : "";

  const complete = async () => {
    await queryClient.invalidateQueries({ queryKey: ["providers"] });
    toast.success(
      t("provider.exchange.success", {
        defaultValue: "已将 {{name}} 的副本放到 {{target}} · {{app}}",
        name,
        target: labelFor(targetId),
        app: APP_DISPLAY_NAME[app],
      }),
    );
    onOpenChange(false);
  };
  const submit = async () => {
    if (!source || !draft || !valid || submitting.current) return;
    submitting.current = true;
    setSaving(true);
    setError(null);
    try {
      // 提交前再校验目标，避免弹窗打开期间新增的同名行被忽略；重试保持同一个 ID。
      const existing = await providersApi.listExchangeTarget(
        app,
        targetId || undefined,
      );
      const previous = existing[draft.id];
      const attempt = JSON.stringify({ scope, name: draft.name, fields });
      if (previous && attemptedCopy.current === attempt) {
        await complete();
        return;
      }
      // 写入成功但响应丢失时，只有原提交能确认完成；改过表单不能复用 ID 覆盖已创建的副本。
      if (previous)
        throw new Error(
          t("provider.exchange.previousCopyExists", {
            defaultValue:
              "上次提交的副本已存在。请关闭弹窗，在目标列表中编辑，或重新发起交换。",
          }),
        );
      if (
        Object.values(existing).some(
          (provider) => provider.name.trim() === name.trim(),
        )
      ) {
        setNames(
          new Set(
            Object.values(existing).map((provider) => provider.name.trim()),
          ),
        );
        setError(
          t("provider.exchange.nameConflict", {
            defaultValue: "目标已有同名供应商，请修改副本名称。",
          }),
        );
        return;
      }
      attemptedCopy.current = attempt;
      const saved = await providersApi.addExchangeCopy(
        draft,
        app,
        targetId || undefined,
      );
      if (!saved)
        throw new Error(
          t("provider.exchange.failed", { defaultValue: "交换失败，请重试。" }),
        );
      await complete();
    } catch (cause) {
      setError(extractErrorMessage(cause));
    } finally {
      submitting.current = false;
      setSaving(false);
    }
  };
  if (!source || preparedSource !== source) return null;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!saving) onOpenChange(open);
      }}
    >
      <DialogContent
        className="max-w-2xl gap-0 p-0"
        onEscapeKeyDown={(event) => {
          if (saving) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <ArrowLeftRight className="h-4 w-4" />
            {t("provider.exchange.title", { defaultValue: "交换供应商" })}
          </DialogTitle>
          <DialogDescription>
            {t("provider.exchange.description", {
              defaultValue:
                "复制当前供应商到指定位置，原供应商保留。副本创建后独立维护，启用时再生效。",
            })}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto px-6 pb-2">
          <div className="flex min-w-0 items-center gap-3 rounded-control bg-subtle px-3 py-2 text-caption">
            <div className="min-w-0 flex-1">
              <div className="text-fg-3">
                {labelFor(source.targetId)} · {APP_DISPLAY_NAME[source.appId]}
              </div>
              <div
                className="truncate font-medium"
                title={source.provider.name}
              >
                {source.provider.name}
              </div>
            </div>
            <ArrowRight className="h-4 w-4 shrink-0 text-fg-3" />
            <div className="min-w-0 flex-1">
              <div className="truncate text-fg-3">
                {labelFor(targetId)} · {APP_DISPLAY_NAME[app]}
              </div>
              <div className="truncate font-medium" title={name}>
                {name}
              </div>
            </div>
          </div>
          <fieldset disabled={saving} className="space-y-4">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor="exchange-target">
                  {t("provider.exchange.target", { defaultValue: "目标环境" })}
                </Label>
                <select
                  id="exchange-target"
                  className={selectClass}
                  value={targetId}
                  onChange={(event) => setTargetId(event.target.value)}
                >
                  <option value="">{labelFor()}</option>
                  {targets.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name} ({target.hostAlias})
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="exchange-app">
                  {t("provider.exchange.app", { defaultValue: "目标应用" })}
                </Label>
                <select
                  id="exchange-app"
                  className={selectClass}
                  value={app}
                  onChange={(event) => {
                    const next = event.target.value as AppId;
                    setApp(next);
                    setFields((current) => ({
                      ...current,
                      apiFormat: exchangeFormats(next).includes(
                        current.apiFormat,
                      )
                        ? current.apiFormat
                        : defaultExchangeFormat(next),
                    }));
                  }}
                >
                  {APP_IDS.map((id) => (
                    <option
                      key={id}
                      value={id}
                      disabled={
                        Boolean(targetId) && !supportsRemoteExchange(id)
                      }
                    >
                      {APP_DISPLAY_NAME[id]}
                    </option>
                  ))}
                </select>
              </div>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="exchange-name">
                {t("provider.exchange.name", { defaultValue: "副本名称" })}
              </Label>
              <Input
                id="exchange-name"
                value={name}
                onChange={(event) => {
                  nameEdited.current = true;
                  setName(event.target.value);
                }}
                aria-invalid={nameConflict}
              />
              {nameConflict && (
                <p className="text-caption text-danger-text">
                  {t("provider.exchange.nameConflict", {
                    defaultValue: "目标已有同名供应商，请修改副本名称。",
                  })}
                </p>
              )}
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="exchange-url">
                {t("provider.exchange.endpoint", {
                  defaultValue: "目标接口地址",
                })}
              </Label>
              <Input
                id="exchange-url"
                value={fields.baseUrl}
                onChange={(event) =>
                  setFields((current) => ({
                    ...current,
                    baseUrl: event.target.value,
                  }))
                }
                placeholder="https://api.example.com/v1"
                aria-invalid={invalidUrl}
              />
              {invalidUrl && (
                <p className="text-caption text-danger-text">
                  {t("provider.exchange.invalidUrl", {
                    defaultValue: "请输入有效的 HTTP 或 HTTPS 接口地址。",
                  })}
                </p>
              )}
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="exchange-key">API Key</Label>
              <div className="flex gap-2">
                <Input
                  id="exchange-key"
                  type={showKey ? "text" : "password"}
                  autoComplete="off"
                  value={fields.apiKey}
                  onChange={(event) =>
                    setFields((current) => ({
                      ...current,
                      apiKey: event.target.value,
                    }))
                  }
                />
                <Button
                  type="button"
                  variant="quiet"
                  size="icon-compact"
                  aria-label={t(
                    showKey
                      ? "provider.exchange.hideKey"
                      : "provider.exchange.showKey",
                    { defaultValue: showKey ? "隐藏密钥" : "显示密钥" },
                  )}
                  onClick={() => setShowKey((value) => !value)}
                >
                  {showKey ? (
                    <EyeOff className="h-4 w-4" />
                  ) : (
                    <Eye className="h-4 w-4" />
                  )}
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor="exchange-model">
                  {t("provider.exchange.model", { defaultValue: "目标模型" })}
                </Label>
                <Input
                  id="exchange-model"
                  value={fields.model}
                  onChange={(event) =>
                    setFields((current) => ({
                      ...current,
                      model: event.target.value,
                    }))
                  }
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="exchange-format">
                  {t("provider.exchange.format", { defaultValue: "上游协议" })}
                </Label>
                <select
                  id="exchange-format"
                  className={selectClass}
                  value={fields.apiFormat}
                  onChange={(event) =>
                    setFields((current) => ({
                      ...current,
                      apiFormat: event.target.value as ExchangeApiFormat,
                    }))
                  }
                >
                  {exchangeFormats(app).map((format) => (
                    <option key={format} value={format}>
                      {formatLabels[format]}
                    </option>
                  ))}
                </select>
              </div>
            </div>
          </fieldset>
          {app !== source.appId && (
            <Notice
              title={t("provider.exchange.conversionHint", {
                defaultValue:
                  "已按目标应用转换连接配置。请确认接口支持所选协议与模型；源应用的专属设置不会跨应用复制。",
              })}
            />
          )}
          {needsAccount && (
            <Notice
              title={t("provider.exchange.accountRequired", {
                defaultValue:
                  "账号登录不能跨应用或机器复制。请填写目标接口地址、模型和 API Key，副本将使用普通 API 认证。",
              })}
            />
          )}
          {missingFields && (
            <p className="text-caption text-fg-3">
              {t("provider.exchange.required", {
                defaultValue:
                  "请补齐目标接口地址和模型；跨环境账号供应商还需要 API Key。",
              })}
            </p>
          )}
          {unsupportedTarget && (
            <p role="alert" className="text-caption text-danger-text">
              {t("provider.exchange.remoteUnsupported", {
                defaultValue:
                  "远程 Agent 暂不支持这个应用，请选择其他应用或本机。",
              })}
            </p>
          )}
          {!sourceFields && (
            <p role="alert" className="text-caption text-danger-text">
              {t("provider.exchange.invalidSource", {
                defaultValue:
                  "源供应商配置无法解析，请先编辑修复源配置，再发起交换。",
              })}
            </p>
          )}
          {loading && (
            <p
              role="status"
              className="flex items-center gap-2 text-caption text-fg-3"
            >
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {t("provider.exchange.loading", {
                defaultValue: "正在读取目标供应商…",
              })}
            </p>
          )}
          {error && (
            <div
              role="alert"
              className="space-y-1 text-caption text-danger-text"
            >
              <p>{error}</p>
              {loadedScope !== scope && (
                <Button
                  variant="quiet"
                  size="compact"
                  onClick={() => setReload((value) => value + 1)}
                >
                  {t("provider.exchange.retry", { defaultValue: "重新读取" })}
                </Button>
              )}
            </div>
          )}
          {draft && (
            <details className="rounded-control border border-border px-3 py-2 text-caption">
              <summary className="cursor-pointer text-fg-2">
                {t("provider.exchange.preview", {
                  defaultValue: "查看目标配置（密钥已隐藏）",
                })}
              </summary>
              <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all font-mono text-fg-3">
                {preview}
              </pre>
            </details>
          )}
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={saving}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
          <Button disabled={!valid || saving} onClick={() => void submit()}>
            {saving && <Loader2 className="mr-1.5 h-4 w-4 animate-spin" />}
            {t("provider.exchange.confirm", {
              defaultValue: "创建副本到 {{app}}",
              app: APP_DISPLAY_NAME[app],
            })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
