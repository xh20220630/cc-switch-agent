import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  ArrowLeftRight,
  Check,
  LoaderCircle,
  Monitor,
  Server,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useRuntimeTarget } from "@/contexts/RuntimeTargetContext";
import { providersApi } from "@/lib/api/providers";
import { providerKeys } from "@/lib/query/queries";
import type { AppId } from "@/lib/api/types";
import type { Provider } from "@/types";
import { deepClone } from "@/utils/deepClone";
import { extractErrorMessage } from "@/utils/errorUtils";

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
  const remoteHostLabel = activeTarget?.hostAlias || activeTarget?.name || "remote";

  const [direction, setDirection] = useState<Direction>("remoteToLocal");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [providers, setProviders] = useState<Record<string, Provider>>({});
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 远程未连接时，默认展示本地→远程
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
        let data: Record<string, Provider>;
        if (direction === "remoteToLocal") {
          if (!isOnline) {
            throw new Error(
              t("provider.sync.remoteOffline", {
                defaultValue: "远程未连接，无法获取远程提供商",
              }),
            );
          }
          data = await providersApi.listRemote(appId);
        } else {
          data = await providersApi.listLocal(appId);
        }
        if (!cancelled) {
          setProviders(data);
          setSelected(new Set());
        }
      } catch (e) {
        if (!cancelled) {
          setError(extractErrorMessage(e));
          setProviders({});
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void fetch();
    return () => {
      cancelled = true;
    };
  }, [open, appId, direction, isOnline, t]);

  const list = useMemo(() => Object.values(providers), [providers]);
  const allSelected = list.length > 0 && selected.size === list.length;

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
    else setSelected(new Set(list.map((p) => p.id)));
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
          defaultValue: `已同步 ${success} 个提供商${failed ? `，${failed} 个失败` : ""}`,
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

  const directionLabel =
    direction === "remoteToLocal"
      ? t("provider.sync.remoteToLocal", {
          defaultValue: `远程 → 本机 (${remoteHostLabel} → local)`,
        })
      : t("provider.sync.localToRemote", {
          defaultValue: `本机 → 远程 (local → ${remoteHostLabel})`,
        });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg gap-0 p-0" zIndex="base">
        <DialogHeader className="px-6 pb-3 pt-6">
          <DialogTitle className="flex items-center gap-2">
            <ArrowLeftRight className="h-5 w-5 text-primary" />
            {t("provider.sync.title", { defaultValue: "同步提供商" })}
          </DialogTitle>
          <DialogDescription>
            {t("provider.sync.description", {
              defaultValue:
                "在本地与远程之间复制提供商。复制后的名称会自动添加来源后缀，便于区分。",
            })}
          </DialogDescription>
        </DialogHeader>

        {/* 方向切换 */}
        <div className="flex gap-2 px-6 pb-3">
          <Button
            variant={direction === "remoteToLocal" ? "default" : "outline"}
            size="sm"
            className="flex-1 gap-1.5"
            disabled={!isOnline && direction !== "remoteToLocal"}
            onClick={() => setDirection("remoteToLocal")}
          >
            <Server className="h-4 w-4" />
            {t("provider.sync.fromRemote", { defaultValue: "远程 → 本机" })}
          </Button>
          <Button
            variant={direction === "localToRemote" ? "default" : "outline"}
            size="sm"
            className="flex-1 gap-1.5"
            onClick={() => setDirection("localToRemote")}
          >
            <Monitor className="h-4 w-4" />
            {t("provider.sync.fromLocal", { defaultValue: "本机 → 远程" })}
          </Button>
        </div>

        <div className="px-6 pb-2 text-xs text-muted-foreground">
          {t("provider.sync.directionHint", {
            defaultValue: `当前方向：${directionLabel}，复制后名称格式为 “原名称(来源)”`,
            directionLabel,
          })}
        </div>

        {!isOnline && direction === "remoteToLocal" ? (
          <div className="mx-6 mb-3 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-900 dark:text-amber-200">
            {t("provider.sync.needOnline", {
              defaultValue: "远程未连接，请先连接远程服务器后再同步。",
            })}
          </div>
        ) : null}

        {/* 列表 */}
        <div className="border-y">
          <div className="flex items-center gap-2 px-6 py-2">
            <Checkbox
              checked={allSelected}
              disabled={list.length === 0 || loading}
              onCheckedChange={toggleAll}
              aria-label={t("provider.sync.selectAll", {
                defaultValue: "全选",
              })}
            />
            <span className="text-sm font-medium">
              {t("provider.sync.selectAll", { defaultValue: "全选" })} ({selected.size}/
              {list.length})
            </span>
            <span className="ml-auto text-xs text-muted-foreground">
              {direction === "remoteToLocal"
                ? t("provider.sync.sourceRemote", {
                    defaultValue: `来源：${remoteHostLabel}`,
                    host: remoteHostLabel,
                  })
                : t("provider.sync.sourceLocal", { defaultValue: "来源：本机" })}
            </span>
          </div>

          <ScrollArea className="h-[280px]">
            <div className="space-y-1 px-2 pb-2">
              {loading ? (
                <div className="flex items-center justify-center gap-2 py-12 text-sm text-muted-foreground">
                  <LoaderCircle className="h-4 w-4 animate-spin" />
                  {t("common.loading", { defaultValue: "加载中..." })}
                </div>
              ) : error ? (
                <div className="py-8 text-center text-sm text-destructive">
                  {error}
                </div>
              ) : list.length === 0 ? (
                <div className="py-8 text-center text-sm text-muted-foreground">
                  {t("provider.sync.empty", {
                    defaultValue: "没有可同步的提供商",
                  })}
                </div>
              ) : (
                list.map((p) => {
                  const checked = selected.has(p.id);
                  const baseUrl = getBaseUrl(p);
                  return (
                    <label
                      key={p.id}
                      className={`flex cursor-pointer items-start gap-3 rounded-md border px-3 py-2.5 transition-colors hover:bg-muted/50 ${checked ? "border-primary/40 bg-primary/5" : "border-transparent bg-card"}`}
                    >
                      <Checkbox
                        checked={checked}
                        onCheckedChange={() => toggleOne(p.id)}
                        className="mt-0.5"
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5">
                          <span className="truncate text-sm font-medium">
                            {p.name}
                          </span>
                          {checked && (
                            <Check className="h-3.5 w-3.5 shrink-0 text-primary" />
                          )}
                        </div>
                        {baseUrl && (
                          <div className="truncate text-xs text-muted-foreground">
                            {baseUrl}
                          </div>
                        )}
                        <div className="mt-1 text-xs text-muted-foreground">
                          {t("provider.sync.copyPreview", {
                            defaultValue: `复制后：${p.name} (${direction === "remoteToLocal" ? remoteHostLabel : "local"})`,
                            name: p.name,
                            suffix:
                              direction === "remoteToLocal"
                                ? remoteHostLabel
                                : "local",
                          })}
                        </div>
                      </div>
                    </label>
                  );
                })
              )}
            </div>
          </ScrollArea>
        </div>

        <DialogFooter className="px-6 py-4">
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
          >
            {syncing && <LoaderCircle className="mr-2 h-4 w-4 animate-spin" />}
            {t("provider.sync.confirm", {
              defaultValue: `同步 ${selected.size} 个`,
              count: selected.size,
            })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
