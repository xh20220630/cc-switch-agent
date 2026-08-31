import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  AlertTriangle,
  ArrowLeftRight,
  ArrowRight,
  Inbox,
  LoaderCircle,
  Lock,
  RefreshCw,
  Search,
  SearchX,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ProviderIcon } from "@/components/ProviderIcon";
import { useRuntimeTarget } from "@/contexts/RuntimeTargetContext";
import { providersApi } from "@/lib/api/providers";
import { providerKeys } from "@/lib/query/queries";
import type { AppId } from "@/lib/api/types";
import type { Provider } from "@/types";
import { cn } from "@/lib/utils";
import { deepClone } from "@/utils/deepClone";
import { extractErrorMessage } from "@/utils/errorUtils";
import { resolveProviderIcon } from "@/utils/providerIcon";

type Direction = "remoteToLocal" | "localToRemote";

interface ProviderSyncDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  appId: AppId;
}

function getBaseUrl(provider: Provider): string {
  const env = (provider.settingsConfig as Record<string, unknown> | undefined)
    ?.env as Record<string, unknown> | undefined;
  const url =
    (env?.ANTHROPIC_BASE_URL as string | undefined) ||
    (env?.OPENAI_BASE_URL as string | undefined) ||
    provider.websiteUrl ||
    "";
  return url;
}

export function ProviderSyncDialog({
  open,
  onOpenChange,
  appId,
}: ProviderSyncDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { snapshot, targets } = useRuntimeTarget();

  const activeTarget = useMemo(
    () => targets.find((tr) => tr.id === snapshot.activeTargetId),
    [targets, snapshot.activeTargetId],
  );
  const isOnline = snapshot.status === "online";
  const remoteHostLabel =
    activeTarget?.hostAlias || activeTarget?.name || "remote";

  const [direction, setDirection] = useState<Direction>("remoteToLocal");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [providers, setProviders] = useState<Record<string, Provider>>({});
  const [targetNames, setTargetNames] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [reloadTick, setReloadTick] = useState(0);

  // 远程未连接时，只允许本机 → 远程
  useEffect(() => {
    if (open && !isOnline && direction === "remoteToLocal") {
      setDirection("localToRemote");
    }
  }, [open, isOnline, direction]);

  useEffect(() => {
    if (!open) {
      setSelected(new Set());
      setError(null);
      return;
    }
    let cancelled = false;
    const fetch = async () => {
      setLoading(true);
      setError(null);
      try {
        const [src, dst] = await Promise.all([
          direction === "remoteToLocal"
            ? providersApi.listRemote(appId)
            : providersApi.listLocal(appId),
          (async () => {
            try {
              return direction === "remoteToLocal"
                ? await providersApi.listLocal(appId)
                : await providersApi.listRemote(appId);
            } catch {
              return {} as Record<string, Provider>;
            }
          })(),
        ]);
        if (!cancelled) {
          setProviders(src);
          setTargetNames(new Set(Object.values(dst).map((p) => p.name)));
          setSelected(new Set());
        }
      } catch (e) {
        if (!cancelled) {
          setError(extractErrorMessage(e));
          setProviders({});
          setTargetNames(new Set());
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void fetch();
    return () => {
      cancelled = true;
    };
  }, [open, appId, direction, isOnline, t, reloadTick]);

  const list = useMemo(() => Object.values(providers), [providers]);

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    if (!query) return list;
    return list.filter(
      (p) =>
        p.name.toLowerCase().includes(query) ||
        getBaseUrl(p).toLowerCase().includes(query) ||
        (p.notes ?? "").toLowerCase().includes(query),
    );
  }, [list, search]);

  // 同步后实际写入的名称后缀（保持原有数据行为）
  const suffix = direction === "remoteToLocal" ? remoteHostLabel : "local";
  // 回程后缀：识别"源自目标侧"的副本，防止往返同步把名称二次叠加
  // （本机→远程时，本地里 `X (prod)` 来自远程，正在送回去；远程→本机同理）
  const roundTripSuffix =
    direction === "remoteToLocal" ? "local" : remoteHostLabel;

  /**
   * 跳过判定（幂等 + 防往返）：
   * - "synced"：目标端已有同名副本 `name (suffix)` → 已同步过，重复同步只会堆积重复条目
   * - "roundTrip"：源端名字已带来源后缀 → 它本来就是目标端的配置，回传无意义
   */
  const skipReason = (p: Provider): "synced" | "roundTrip" | null => {
    if (targetNames.has(`${p.name} (${suffix})`)) return "synced";
    if (p.name.endsWith(` (${roundTripSuffix})`)) return "roundTrip";
    return null;
  };

  const selectable = filtered.filter((p) => !skipReason(p));
  const allSelected =
    selectable.length > 0 && selected.size === selectable.length;
  const skipCount = filtered.length - selectable.length;

  const toggleOne = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAll = () => {
    if (allSelected) setSelected(new Set());
    else setSelected(new Set(selectable.map((p) => p.id)));
  };

  const handleSync = async () => {
    if (selected.size === 0) return;
    setSyncing(true);
    const suffix = direction === "remoteToLocal" ? remoteHostLabel : "local";
    const sourceList = providers;
    let success = 0;
    let failed = 0;
    const errors: string[] = [];

    for (const id of selected) {
      const src = sourceList[id];
      if (!src) continue;
      // 防御：已同步过 / 回程副本即使在选中集合里也跳过，避免产生重复条目
      if (skipReason(src)) continue;
      const cloned: Provider = {
        ...deepClone(src),
        id: crypto.randomUUID(),
        name: `${src.name} (${suffix})`,
        createdAt: Date.now(),
        inFailoverQueue: false,
        meta: (() => {
          if (!src.meta) return undefined;
          const m = { ...src.meta } as Record<string, unknown>;
          delete m.remote_synced;
          delete m.remoteSynced;
          return m as typeof src.meta;
        })(),
      };
      try {
        if (direction === "remoteToLocal") {
          await providersApi.addToLocal(cloned, appId);
        } else {
          await providersApi.addToRemote(cloned, appId);
        }
        success += 1;
      } catch (e) {
        failed += 1;
        errors.push(`${src.name}: ${extractErrorMessage(e)}`);
      }
    }

    setSyncing(false);
    if (success > 0) {
      // 刷新提供商列表（本地与远程都会因重新获取而更新；统一失效）
      await queryClient.invalidateQueries({
        queryKey: providerKeys.byApp(appId),
      });
      if (direction === "localToRemote") {
        // 远程写入后，远程列表也会变化
        await queryClient.invalidateQueries({ queryKey: ["providers"] });
      }
      toast.success(
        t("provider.sync.success", {
          defaultValue: `已同步 ${success} 个 Provider${failed ? `，${failed} 个失败` : ""}`,
          count: success,
        }),
      );
      if (failed > 0 && errors.length > 0) {
        toast.error(errors.slice(0, 3).join("\n"));
      }
      onOpenChange(false);
    } else if (failed > 0) {
      toast.error(
        t("provider.sync.failed", { defaultValue: "同步失败" }) +
          (errors[0] ? `: ${errors[0]}` : ""),
      );
    }
  };

  // —— 方向与文案（单一方向表达）——
  const localLabel = t("provider.sync.targetLocal", { defaultValue: "本机" });
  const remoteLabel = t("provider.sync.targetRemote", { defaultValue: "远程" });
  const fromLabel = direction === "remoteToLocal" ? remoteLabel : localLabel;
  const toLabel = direction === "remoteToLocal" ? localLabel : remoteLabel;
  const targetLabel = toLabel;
  const directionDesc =
    direction === "remoteToLocal"
      ? t("provider.sync.directionDescRemoteToLocal", {
          defaultValue: "将远程的 Provider 同步到本机环境",
        })
      : t("provider.sync.directionDescLocalToRemote", {
          defaultValue: "将本机的 Provider 同步到远程环境",
        });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl gap-0 p-0" zIndex="base">
        <DialogHeader className="px-6 py-4">
          <DialogTitle className="flex items-center gap-2">
            <span className="flex h-6 w-6 items-center justify-center rounded-md bg-primary/10 text-primary">
              <ArrowLeftRight className="h-4 w-4" />
            </span>
            {t("provider.sync.title", { defaultValue: "同步 Provider" })}
          </DialogTitle>
        </DialogHeader>

        {/* 方向声明：唯一的方向表达 */}
        <div className="flex items-start justify-between gap-3 px-6 pt-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <span className="text-muted-foreground">{fromLabel}</span>
              <ArrowRight className="h-4 w-4 shrink-0 text-primary" />
              <span>{toLabel}</span>
            </div>
            <p className="mt-1 text-xs text-muted-foreground">
              {directionDesc}
            </p>
          </div>
          <button
            type="button"
            disabled={!isOnline}
            onClick={() =>
              setDirection((d) =>
                d === "remoteToLocal" ? "localToRemote" : "remoteToLocal",
              )
            }
            title={
              isOnline
                ? t("provider.sync.swapDirection", { defaultValue: "切换方向" })
                : t("provider.sync.remoteUnavailable", {
                    defaultValue: "远程未连接，暂时无法从远程同步",
                  })
            }
            aria-label={t("provider.sync.swapDirection", {
              defaultValue: "切换方向",
            })}
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
          >
            {isOnline ? (
              <ArrowLeftRight className="h-4 w-4" />
            ) : (
              <Lock className="h-4 w-4" />
            )}
          </button>
        </div>

        {/* 搜索 */}
        <div className="px-6 pt-4">
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("provider.sync.searchPlaceholder", {
                defaultValue: "搜索 Provider 名称或接口地址...",
              })}
              className="h-9 pl-8"
            />
          </div>
        </div>

        {/* 全选 + 已选择计数 */}
        <div className="flex items-center justify-between px-6 pt-4">
          <label className="flex cursor-pointer select-none items-center gap-2 text-sm">
            <Checkbox
              checked={allSelected}
              disabled={selectable.length === 0 || loading}
              onCheckedChange={toggleAll}
              aria-label={t("provider.sync.selectAll", {
                defaultValue: "全选",
              })}
            />
            {t("provider.sync.selectAll", { defaultValue: "全选" })}
          </label>
          <span className="text-xs text-muted-foreground">
            {t("provider.sync.selectedCount", {
              defaultValue: `已选择 ${selected.size} / ${selectable.length}`,
              count: selected.size,
              total: selectable.length,
            })}
          </span>
        </div>

        {/* 列表：轻量行，整行可点 */}
        <div className="mx-4 mt-4 mb-4 overflow-hidden rounded-lg border border-border/60 bg-muted/10">
          <ScrollArea className="h-[320px]">
            <div className="p-1">
              {loading ? (
                <div className="flex items-center justify-center gap-2 py-14 text-sm text-muted-foreground">
                  <LoaderCircle className="h-4 w-4 animate-spin" />
                  {t("common.loading", { defaultValue: "加载中..." })}
                </div>
              ) : error ? (
                <div className="flex flex-col items-center gap-2 py-10 text-center">
                  <AlertTriangle className="h-5 w-5 text-destructive" />
                  <p className="max-w-full break-words px-4 text-sm text-destructive">
                    {error}
                  </p>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setReloadTick((v) => v + 1)}
                  >
                    <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
                    {t("provider.sync.retry", { defaultValue: "重试" })}
                  </Button>
                </div>
              ) : filtered.length === 0 ? (
                search.trim() ? (
                  <div className="flex flex-col items-center gap-1.5 py-12 text-center">
                    <SearchX className="h-5 w-5 text-muted-foreground/60" />
                    <p className="text-sm text-muted-foreground">
                      {t("provider.sync.noResults", {
                        defaultValue: "没有匹配的 Provider",
                      })}
                    </p>
                  </div>
                ) : (
                  <div className="flex flex-col items-center gap-1.5 py-12 text-center">
                    <Inbox className="h-5 w-5 text-muted-foreground/60" />
                    <p className="text-sm text-muted-foreground">
                      {t("provider.sync.empty", {
                        defaultValue: "没有可同步的 Provider",
                      })}
                    </p>
                  </div>
                )
              ) : (
                filtered.map((p) => {
                  const checked = selected.has(p.id);
                  const baseUrl = getBaseUrl(p);
                  const sameNameOnTarget = targetNames.has(p.name);
                  const previewName = `${p.name} (${suffix})`;
                  const skip = skipReason(p);
                  if (skip) {
                    return (
                      <div
                        key={p.id}
                        className="flex select-none items-center gap-3 rounded-md px-3 py-2 opacity-50"
                      >
                        <Checkbox
                          checked={false}
                          disabled
                          className="shrink-0"
                          tabIndex={-1}
                        />
                        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-muted">
                          <ProviderIcon
                            icon={resolveProviderIcon(
                              appId,
                              p.icon,
                              p.iconColor,
                            )}
                            name={p.name}
                            size={14}
                          />
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm text-foreground/90">
                            {p.name}
                          </span>
                          {baseUrl && (
                            <span className="block truncate text-xs text-muted-foreground">
                              {baseUrl}
                            </span>
                          )}
                          <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                            {skip === "synced"
                              ? t("provider.sync.alreadySynced", {
                                  defaultValue: `已同步过，将跳过`,
                                })
                              : t("provider.sync.fromTargetSkipped", {
                                  defaultValue: `来自${targetLabel}，将跳过`,
                                  target: targetLabel,
                                })}
                          </span>
                        </span>
                      </div>
                    );
                  }
                  return (
                    <label
                      key={p.id}
                      className={cn(
                        "relative flex cursor-pointer select-none items-center gap-3 rounded-md px-3 py-2 transition-colors",
                        checked ? "bg-primary/5" : "hover:bg-muted/50",
                      )}
                    >
                      {checked && (
                        <span className="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full bg-primary" />
                      )}
                      <Checkbox
                        checked={checked}
                        onCheckedChange={() => toggleOne(p.id)}
                        className="shrink-0"
                      />
                      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-muted">
                        <ProviderIcon
                          icon={resolveProviderIcon(appId, p.icon, p.iconColor)}
                          name={p.name}
                          size={14}
                        />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span
                          className={cn(
                            "block truncate text-sm",
                            checked ? "font-medium" : "text-foreground/90",
                          )}
                        >
                          {p.name}
                        </span>
                        {baseUrl && (
                          <span className="block truncate text-xs text-muted-foreground">
                            {baseUrl}
                          </span>
                        )}
                        {sameNameOnTarget && (
                          <span className="mt-0.5 block truncate text-xs text-amber-600 dark:text-amber-400">
                            {t("provider.sync.conflictNote", {
                              defaultValue: `${targetLabel}已存在同名配置，将保存为 ${previewName}`,
                              target: targetLabel,
                              newName: previewName,
                            })}
                          </span>
                        )}
                      </span>
                    </label>
                  );
                })
              )}
            </div>
          </ScrollArea>
        </div>

        {/* 摘要 + 操作 */}
        <DialogFooter className="items-center gap-2 px-6 py-4">
          <span className="mr-auto text-xs text-muted-foreground">
            {selected.size > 0
              ? t("provider.sync.summary", {
                  defaultValue: `${selected.size} 个 Provider 将同步到${targetLabel}`,
                  count: selected.size,
                  target: targetLabel,
                })
              : t("provider.sync.noneSelected", {
                  defaultValue: "请选择要同步的 Provider",
                })}
            {skipCount > 0 && (
              <span className="ml-2">
                {t("provider.sync.skippedHint", {
                  defaultValue: `（${skipCount} 个已同步过，将跳过）`,
                  count: skipCount,
                })}
              </span>
            )}
          </span>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={syncing}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
          <Button
            onClick={handleSync}
            disabled={selected.size === 0 || syncing || loading}
            className="min-w-[7rem]"
          >
            {syncing && <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />}
            {t("provider.sync.confirm", {
              defaultValue: `同步到${targetLabel}`,
              target: targetLabel,
            })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
