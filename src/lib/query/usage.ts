import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { usageApi } from "@/lib/api/usage";
import { resolveUsageRange } from "@/lib/usageRange";
import {
  runtimeQueryScope,
  useRuntimeQueryScope,
  type RuntimeQueryScope,
} from "@/lib/runtime/queryScope";
import type {
  LogFilters,
  UsageRangeSelection,
  UsageScopeFilters,
} from "@/types/usage";

/** 时间/筛选变化保留当前主机的旧结果；换主机时清空占位数据，避免短暂展示其他环境用量。 */
function scopedUsagePlaceholder<T>(
  previousData: T | undefined,
  previousQuery?: { queryKey: readonly unknown[] },
): T | undefined {
  const prefix = usageKeys.all();
  return prefix.every((part, index) => previousQuery?.queryKey[index] === part)
    ? previousData
    : undefined;
}

const DEFAULT_REFETCH_INTERVAL_MS = 30000;

type UsageQueryOptions = {
  enabled?: boolean;
  refetchInterval?: number | false;
  refetchIntervalInBackground?: boolean;
};

type RequestLogsQueryArgs = {
  filters: LogFilters;
  range: UsageRangeSelection;
  page?: number;
  pageSize?: number;
  options?: UsageQueryOptions;
};

type RequestLogsKey = {
  preset: UsageRangeSelection["preset"];
  customStartDate?: number;
  customEndDate?: number;
  liveEndTime?: boolean;
  appType?: string;
  providerName?: string;
  model?: string;
  statusCode?: number;
};

// Query keys
export const usageKeys = {
  // scope 紧跟 domain root，允许按 ["usage", ...scope] 精确失效当前主机，
  // 同时保留 ["usage"] 作为维护工具清理全部环境缓存的公共前缀。
  all: (scope: RuntimeQueryScope = runtimeQueryScope()) =>
    ["usage", ...scope] as const,
  summary: (
    preset: UsageRangeSelection["preset"],
    customStartDate: number | undefined,
    customEndDate: number | undefined,
    filters?: UsageScopeFilters,
    liveEndTime?: boolean,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "summary",
      preset,
      customStartDate ?? 0,
      customEndDate ?? 0,
      liveEndTime ?? false,
      filters?.appType ?? null,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  summaryByApp: (
    preset: UsageRangeSelection["preset"],
    customStartDate: number | undefined,
    customEndDate: number | undefined,
    filters?: Pick<UsageScopeFilters, "providerName" | "model">,
    liveEndTime?: boolean,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "summary-by-app",
      preset,
      customStartDate ?? 0,
      customEndDate ?? 0,
      liveEndTime ?? false,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  trends: (
    preset: UsageRangeSelection["preset"],
    customStartDate: number | undefined,
    customEndDate: number | undefined,
    filters?: UsageScopeFilters,
    liveEndTime?: boolean,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "trends",
      preset,
      customStartDate ?? 0,
      customEndDate ?? 0,
      liveEndTime ?? false,
      filters?.appType ?? null,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  firstDate: (
    filters?: UsageScopeFilters,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "first-date",
      filters?.appType ?? null,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  providerStats: (
    preset: UsageRangeSelection["preset"],
    customStartDate: number | undefined,
    customEndDate: number | undefined,
    filters?: UsageScopeFilters,
    liveEndTime?: boolean,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "provider-stats",
      preset,
      customStartDate ?? 0,
      customEndDate ?? 0,
      liveEndTime ?? false,
      filters?.appType ?? null,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  modelStats: (
    preset: UsageRangeSelection["preset"],
    customStartDate: number | undefined,
    customEndDate: number | undefined,
    filters?: UsageScopeFilters,
    liveEndTime?: boolean,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "model-stats",
      preset,
      customStartDate ?? 0,
      customEndDate ?? 0,
      liveEndTime ?? false,
      filters?.appType ?? null,
      filters?.providerName ?? null,
      filters?.model ?? null,
    ] as const,
  logs: (
    key: RequestLogsKey,
    page: number,
    pageSize: number,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) =>
    [
      ...usageKeys.all(scope),
      "logs",
      key.preset,
      key.customStartDate ?? 0,
      key.customEndDate ?? 0,
      key.liveEndTime ?? false,
      key.appType ?? "",
      key.providerName ?? "",
      key.model ?? "",
      key.statusCode ?? -1,
      page,
      pageSize,
    ] as const,
  detail: (requestId: string, scope: RuntimeQueryScope = runtimeQueryScope()) =>
    [...usageKeys.all(scope), "detail", requestId] as const,
  dataSources: (scope: RuntimeQueryScope = runtimeQueryScope()) =>
    [...usageKeys.all(scope), "data-sources"] as const,
  pricing: (scope: RuntimeQueryScope = runtimeQueryScope()) =>
    [...usageKeys.all(scope), "pricing"] as const,
  limits: (
    providerId: string,
    appType: string,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) => [...usageKeys.all(scope), "limits", providerId, appType] as const,
  script: (
    providerId: string,
    appType: string,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) => [...usageKeys.all(scope), "script", providerId, appType] as const,
  // 会话汇总也按主机隔离，阅读页不能复用另一台主机的同名会话。
  session: (
    appType: string,
    sessionId: string,
    scope: RuntimeQueryScope = runtimeQueryScope(),
  ) => [...usageKeys.all(scope), "session", appType, sessionId] as const,
};

/** 把 UI 侧的 "all" 哨兵归一成 undefined（后端语义：不过滤）。 */
function normalizeScopeFilters(filters?: UsageScopeFilters): UsageScopeFilters {
  return {
    appType: filters?.appType === "all" ? undefined : filters?.appType,
    providerName: filters?.providerName,
    model: filters?.model,
  };
}

// Hooks
// 统计类查询都带 keepPreviousData：换筛选、时间范围、翻页时先留着上一份数据，
// 新数据到了再换，不让指标闪成「…」、请求日志表塌成骨架。
export function useUsageSummary(
  range: UsageRangeSelection,
  filters?: UsageScopeFilters,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  const effective = normalizeScopeFilters(filters);
  return useQuery({
    queryKey: usageKeys.summary(
      range.preset,
      range.customStartDate,
      range.customEndDate,
      effective,
      range.liveEndTime,
      scope,
    ),
    queryFn: () => {
      const { startDate, endDate } = resolveUsageRange(range);
      return usageApi.getUsageSummary(
        startDate,
        endDate,
        effective.appType,
        effective.providerName,
        effective.model,
      );
    },
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useUsageSummaryByApp(
  range: UsageRangeSelection,
  filters?: Pick<UsageScopeFilters, "providerName" | "model">,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  return useQuery({
    queryKey: usageKeys.summaryByApp(
      range.preset,
      range.customStartDate,
      range.customEndDate,
      filters,
      range.liveEndTime,
      scope,
    ),
    queryFn: () => {
      const { startDate, endDate } = resolveUsageRange(range);
      return usageApi.getUsageSummaryByApp(
        startDate,
        endDate,
        filters?.providerName,
        filters?.model,
      );
    },
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useUsageTrends(
  range: UsageRangeSelection,
  filters?: UsageScopeFilters,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  const effective = normalizeScopeFilters(filters);
  return useQuery({
    queryKey: usageKeys.trends(
      range.preset,
      range.customStartDate,
      range.customEndDate,
      effective,
      range.liveEndTime,
      scope,
    ),
    queryFn: () => {
      const { startDate, endDate } = resolveUsageRange(range);
      return usageApi.getUsageTrends(
        startDate,
        endDate,
        effective.appType,
        effective.providerName,
        effective.model,
      );
    },
    enabled: options?.enabled ?? true,
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

/** 最早有用量记录的日期，「全部」的按年热力图据此决定从哪一年画起 */
export function useUsageFirstDate(
  filters?: UsageScopeFilters,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  const effective = normalizeScopeFilters(filters);
  return useQuery({
    // 热力图年份必须随当前操作主机切换，不能沿用上一个主机的历史范围。
    queryKey: usageKeys.firstDate(effective, scope),
    queryFn: () =>
      usageApi.getUsageFirstDate(
        effective.appType,
        effective.providerName,
        effective.model,
      ),
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useProviderStats(
  range: UsageRangeSelection,
  filters?: UsageScopeFilters,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  const effective = normalizeScopeFilters(filters);
  return useQuery({
    queryKey: usageKeys.providerStats(
      range.preset,
      range.customStartDate,
      range.customEndDate,
      effective,
      range.liveEndTime,
      scope,
    ),
    queryFn: () => {
      const { startDate, endDate } = resolveUsageRange(range);
      return usageApi.getProviderStats(
        startDate,
        endDate,
        effective.appType,
        effective.providerName,
        effective.model,
      );
    },
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useModelStats(
  range: UsageRangeSelection,
  filters?: UsageScopeFilters,
  options?: UsageQueryOptions,
) {
  const scope = useRuntimeQueryScope();
  const effective = normalizeScopeFilters(filters);
  return useQuery({
    queryKey: usageKeys.modelStats(
      range.preset,
      range.customStartDate,
      range.customEndDate,
      effective,
      range.liveEndTime,
      scope,
    ),
    queryFn: () => {
      const { startDate, endDate } = resolveUsageRange(range);
      return usageApi.getModelStats(
        startDate,
        endDate,
        effective.appType,
        effective.providerName,
        effective.model,
      );
    },
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useRequestLogs({
  filters,
  range,
  page = 0,
  pageSize = 20,
  options,
}: RequestLogsQueryArgs) {
  const scope = useRuntimeQueryScope();
  const key: RequestLogsKey = {
    preset: range.preset,
    customStartDate: range.customStartDate,
    customEndDate: range.customEndDate,
    liveEndTime: range.liveEndTime,
    appType: filters.appType,
    providerName: filters.providerName,
    model: filters.model,
    statusCode: filters.statusCode,
  };

  return useQuery({
    queryKey: usageKeys.logs(key, page, pageSize, scope),
    queryFn: () => {
      const effectiveFilters = { ...filters, ...resolveUsageRange(range) };
      return usageApi.getRequestLogs(effectiveFilters, page, pageSize);
    },
    placeholderData: scopedUsagePlaceholder,
    refetchInterval: options?.refetchInterval ?? DEFAULT_REFETCH_INTERVAL_MS, // 每30秒自动刷新
    refetchIntervalInBackground: options?.refetchIntervalInBackground ?? false,
  });
}

export function useRequestDetail(requestId: string) {
  const scope = useRuntimeQueryScope();
  return useQuery({
    queryKey: usageKeys.detail(requestId, scope),
    queryFn: () => usageApi.getRequestDetail(requestId),
    enabled: !!requestId,
  });
}

/**
 * 会话日志扫描（后台定时或手动同步）最近一次完成的时间（毫秒）。
 * 后台每 60 秒扫一次，这里 30 秒问一次；挂在 usage 下，同步后跟着失效重取。
 */
export function useSessionUsageLastSync() {
  const scope = useRuntimeQueryScope();
  return useQuery({
    // 后台扫描进度属于当前主机，切目标时必须读取对应 Agent 的扫描时间。
    queryKey: [...usageKeys.all(scope), "session-last-sync"] as const,
    queryFn: () => usageApi.getSessionUsageLastSync(),
    refetchInterval: DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: false,
  });
}

/** 单个会话的用量汇总（会话阅读页头部），只数会话日志导入的行。 */
export function useSessionUsageSummary(appType: string, sessionId: string) {
  const scope = useRuntimeQueryScope();
  return useQuery({
    queryKey: usageKeys.session(appType, sessionId, scope),
    queryFn: () => usageApi.getSessionUsageSummary(appType, sessionId),
    refetchInterval: DEFAULT_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: false,
  });
}

export function useModelPricing() {
  const scope = useRuntimeQueryScope();
  return useQuery({
    queryKey: usageKeys.pricing(scope),
    queryFn: usageApi.getModelPricing,
  });
}

export function useProviderLimits(providerId: string, appType: string) {
  const scope = useRuntimeQueryScope();
  return useQuery({
    queryKey: usageKeys.limits(providerId, appType, scope),
    queryFn: () => usageApi.checkProviderLimits(providerId, appType),
    enabled: !!providerId && !!appType,
  });
}

export function useUpdateModelPricing() {
  const queryClient = useQueryClient();
  const scope = useRuntimeQueryScope();

  return useMutation({
    mutationFn: (params: {
      modelId: string;
      displayName: string;
      inputCost: string;
      outputCost: string;
      cacheReadCost: string;
      cacheCreationCost: string;
    }) =>
      usageApi.updateModelPricing(
        params.modelId,
        params.displayName,
        params.inputCost,
        params.outputCost,
        params.cacheReadCost,
        params.cacheCreationCost,
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: usageKeys.all(scope) });
    },
  });
}

export function useDeleteModelPricing() {
  const queryClient = useQueryClient();
  const scope = useRuntimeQueryScope();

  return useMutation({
    mutationFn: (modelId: string) => usageApi.deleteModelPricing(modelId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: usageKeys.all(scope) });
    },
  });
}
