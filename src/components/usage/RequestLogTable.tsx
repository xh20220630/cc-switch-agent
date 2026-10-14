import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useRequestLogs } from "@/lib/query/usage";
import { TablePagination } from "./TablePagination";
import { HelpTip } from "@/components/ui/help-tip";
import { AppGlyph, APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import type { AppId } from "@/lib/api";
import {
  getFreshInputTokens,
  isUnpricedUsage,
  type LogFilters,
  type RequestLog,
  type UsageRangeSelection,
} from "@/types/usage";
import { cn } from "@/lib/utils";
import {
  fmtInt,
  fmtUsd,
  formatFirstTokenSeconds,
  formatTokensCompact,
  formatTokensPerSecond,
  getAggregateTokensPerSecond,
  getLocaleFromLanguage,
  getRequestTimingMetrics,
  parseFiniteNumber,
  summarizeRequestLogSpeeds,
} from "./format";
import { usageTable } from "./usageTable";
import { getUsageProviderLabel, usageProviderTitle } from "./providerLabel";

interface RequestLogTableProps {
  range: UsageRangeSelection;
  /** 旧接口保留；时间范围由页面顶部的筛选统一控制 */
  rangeLabel?: string;
  appType?: string;
  providerName?: string;
  model?: string;
  /** 状态码筛选（页签行右侧的下拉） */
  statusCode?: number;
  refreshIntervalMs: number;
  /** 点一行打开请求详情 */
  onOpenDetail?: (requestId: string) => void;
}

const pad2 = (value: number) => String(value).padStart(2, "0");

const isSameLocalDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();

/**
 * 表格里的时间：今天（本地时区）只显示时刻「14:32:05」，
 * 别的日子显示「09-30 14:21」。完整时间放在悬停提示和详情抽屉里。
 */
export function formatLogTime(createdAt: number, now = new Date()): string {
  const date = new Date(createdAt * 1000);
  const clock = `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
  if (isSameLocalDay(date, now)) {
    return `${clock}:${pad2(date.getSeconds())}`;
  }
  return `${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ${clock}`;
}

/** 「2026-09-30 14:21:05」：悬停时看完整的本地时间。 */
export function formatLogFullTime(createdAt: number): string {
  const date = new Date(createdAt * 1000);
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(
    date.getDate(),
  )} ${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(
    date.getSeconds(),
  )}`;
}

export function isKnownAppId(appType: string): appType is AppId {
  return appType in APP_DISPLAY_NAME;
}

export function appDisplayName(appType: string): string {
  return isKnownAppId(appType) ? APP_DISPLAY_NAME[appType] : appType;
}

/**
 * 请求日志「应用」列用的短名：图标已经区分了品牌（Claude Code / Desktop 靠角标），
 * 列里只留最短能认出的名字，把宽度让给供应商列。全名在悬停提示里。
 */
const APP_SHORT_NAME: Record<AppId, string> = {
  claude: "Claude",
  "claude-desktop": "Desktop",
  codex: "Codex",
  gemini: "Gemini",
  grokbuild: "Grok",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  hermes: "Hermes",
  pi: "Pi",
  mcode: "MiniMax",
};

export function appShortName(appType: string): string {
  return isKnownAppId(appType) ? APP_SHORT_NAME[appType] : appType;
}

const isSuccessStatus = (code: number) => code >= 200 && code < 300;

export function RequestLogTable({
  range,
  appType: dashboardAppType,
  providerName,
  model,
  statusCode,
  refreshIntervalMs,
  onOpenDetail,
}: RequestLogTableProps) {
  const { t, i18n } = useTranslation();
  const [page, setPage] = useState(0);
  const pageSize = 20;

  const effectiveFilters: LogFilters = {
    appType:
      dashboardAppType && dashboardAppType !== "all"
        ? dashboardAppType
        : undefined,
    providerName,
    model,
    statusCode,
  };

  const { data: result, isLoading } = useRequestLogs({
    filters: effectiveFilters,
    range,
    page,
    pageSize,
    options: {
      refetchInterval: refreshIntervalMs > 0 ? refreshIntervalMs : false,
    },
  });

  const logs = result?.data ?? [];
  const total = result?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  // 新后端返回全筛选范围的汇总；旧 Agent 只展示本页，明确标明范围，避免翻页误报总速度。
  const speedSummary = result?.speedSummary ?? summarizeRequestLogSpeeds(logs);
  const isFullSummary = result?.speedSummary != null || total === logs.length;
  const generationSpeed = formatTokensPerSecond(
    getAggregateTokensPerSecond(
      speedSummary.generationOutputTokens,
      speedSummary.generationDurationMs,
    ),
  );
  const totalSpeed = formatTokensPerSecond(
    getAggregateTokensPerSecond(
      speedSummary.outputTokens,
      speedSummary.totalDurationMs,
    ),
  );

  useEffect(() => {
    setPage(0);
  }, [
    dashboardAppType,
    providerName,
    model,
    statusCode,
    range.customEndDate,
    range.customStartDate,
    range.preset,
  ]);

  const language = i18n.resolvedLanguage || i18n.language || "en";
  const locale = getLocaleFromLanguage(language);
  // 「今天」按本地时区判断；每次渲染（含自动刷新）取一次当前时间
  const now = new Date();

  if (isLoading) {
    return <div className={usageTable.skeleton} />;
  }

  const renderRow = (log: RequestLog) => {
    const unpriced = isUnpricedUsage(log);
    const freshInput = getFreshInputTokens(log);
    const isCacheInclusive = log.inputTokens !== freshInput;
    const time = formatLogTime(log.createdAt, now);
    const fullTime = formatLogFullTime(log.createdAt);
    const providerLabel = getUsageProviderLabel(log.providerName, t);
    const provider = providerLabel.shortLabel;
    const timing = getRequestTimingMetrics(log);
    const firstTokenText = formatFirstTokenSeconds(timing.firstTokenMs);
    const durationText = formatFirstTokenSeconds(timing.totalMs);
    const generationTps = formatTokensPerSecond(timing.generationTps);
    const totalTps = formatTokensPerSecond(timing.totalTps);
    // 明细区直接展示两种口径；估算来源始终保留 ≈，没有首字时只展示全程速度。
    const approximate = timing.isEstimated ? "≈" : "";
    const timingTip = t("usage.requestTimingTip", {
      first: firstTokenText ?? t("usage.timingNotRecorded"),
      duration: durationText ?? t("usage.timingNotRecorded"),
    });
    const multiplier = parseFiniteNumber(log.costMultiplier);
    const modelTitle =
      log.requestModel && log.requestModel !== log.model
        ? `${log.requestModel} → ${log.model}`
        : log.model;
    const hasCache = log.cacheReadTokens > 0;

    return (
      <tr
        key={log.requestId}
        className={cn(
          onOpenDetail ? usageTable.rowInteractive : usageTable.row,
          "group",
        )}
        onClick={() => onOpenDetail?.(log.requestId)}
      >
        {/* 时间、应用两列收紧到内容宽度（w-px），多出来的宽度留给后面的数值列 */}
        <td className={cn(usageTable.td, "w-px")}>
          <button
            type="button"
            className="rounded-[4px] text-start tabular-nums focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            title={fullTime}
            aria-label={t("usage.openRequestDetail", {
              time: fullTime,
              provider,
            })}
            onClick={(event) => {
              event.stopPropagation();
              onOpenDetail?.(log.requestId);
            }}
          >
            {time}
          </button>
          {!isSuccessStatus(log.statusCode) && (
            <span
              className="ms-1.5 rounded-[4px] bg-danger-soft px-1 text-badge text-danger-text"
              title={log.errorMessage || undefined}
            >
              {log.statusCode}
            </span>
          )}
        </td>
        <td className={cn(usageTable.td, "w-px")}>
          <span
            className="flex max-w-[88px] items-center gap-1.5"
            title={appDisplayName(log.appType)}
          >
            {isKnownAppId(log.appType) && (
              <AppGlyph
                app={log.appType}
                size={14}
                badgeClassName="bg-surface"
              />
            )}
            <span className="truncate" aria-hidden="true">
              {appShortName(log.appType)}
            </span>
            <span className="sr-only">{appDisplayName(log.appType)}</span>
          </span>
        </td>
        {/* 供应商、模型两列按比例取宽（max-w-0 让百分比宽度生效、内容截断）；
            比例合计 32%：40% 时常见窗口下数值列就只剩内容宽度，左松右挤。模型名通常比供应商名长，
            放不下的截断、悬停看全名 */}
        <td className={cn(usageTable.td, "w-[14%] max-w-0")}>
          <span
            className="block truncate"
            title={usageProviderTitle(providerLabel)}
          >
            {provider}
          </span>
        </td>
        <td className={cn(usageTable.td, usageTable.mono, "w-[18%] max-w-0")}>
          <span className="block truncate" title={modelTitle}>
            {log.model}
          </span>
        </td>
        <td
          className={usageTable.td}
          title={
            isCacheInclusive
              ? `${fmtInt(freshInput, locale)} (${t("usage.rawInputLabel")}: ${fmtInt(log.inputTokens, locale)})`
              : fmtInt(freshInput, locale)
          }
        >
          {formatTokensCompact(freshInput, locale)}
        </td>
        <td className={usageTable.td} title={fmtInt(log.outputTokens, locale)}>
          {formatTokensCompact(log.outputTokens, locale)}
        </td>
        <td
          className={cn(usageTable.td, !hasCache && usageTable.muted)}
          title={t("usage.cacheTip", {
            read: fmtInt(log.cacheReadTokens, locale),
            write: fmtInt(log.cacheCreationTokens, locale),
          })}
        >
          {hasCache ? formatTokensCompact(log.cacheReadTokens, locale) : "—"}
        </td>
        <td
          className={cn(
            usageTable.td,
            "font-medium",
            unpriced && "font-normal text-fg-3",
          )}
          title={
            multiplier != null && multiplier !== 1
              ? `${t("usage.costMultiplier")} ×${multiplier.toFixed(2)}`
              : undefined
          }
        >
          {unpriced ? t("usage.unpriced") : fmtUsd(log.totalCostUsd, 4)}
        </td>
        {/* 两个计时列一起固定；末列内宽 122px 加两侧 padding 为 136px，须与此偏移同步维护。 */}
        <td
          className={cn(
            usageTable.td,
            "sticky end-[136px] z-10 bg-surface group-hover:bg-subtle",
          )}
          title={timingTip}
        >
          <div className="w-[100px] py-1 text-caption leading-5">
            <div>
              <span className="me-1 text-fg-3">{t("usage.firstToken")}</span>
              {firstTokenText == null ? "—" : `${approximate}${firstTokenText}`}
            </div>
            <div>
              <span className="me-1 text-fg-3">{t("usage.totalDuration")}</span>
              {durationText == null ? "—" : `${approximate}${durationText}`}
            </div>
          </div>
        </td>
        {/* 速度列固定在右侧，窄窗口横向滚动时仍能看到当前请求的两种速率。 */}
        <td
          className={cn(
            usageTable.td,
            "sticky end-0 z-10 bg-surface group-hover:bg-subtle",
          )}
          title={
            timing.totalMs == null
              ? t("usage.requestTimingMissing")
              : t("usage.requestSpeedHelp")
          }
        >
          <div className="w-[122px] whitespace-normal py-1 text-caption leading-5">
            <div className={cn(generationTps == null && usageTable.muted)}>
              <span className="me-1 text-fg-3">
                {t("usage.afterFirstToken")}
              </span>
              {generationTps == null
                ? "—"
                : `${approximate}${generationTps} tok/s`}
            </div>
            <div className={cn(totalTps == null && usageTable.muted)}>
              <span className="me-1 text-fg-3">{t("usage.endToEnd")}</span>
              {totalTps == null ? "—" : `${approximate}${totalTps} tok/s`}
            </div>
          </div>
        </td>
      </tr>
    );
  };

  return (
    <div className="flex flex-col">
      <div className={usageTable.scroller}>
        {/* 耗时与速度各用两行，保持列数；速度固定在右侧，不会随供应商长名称被挤出视野。 */}
        <table
          className={cn(usageTable.table, "min-w-[620px]")}
          aria-label={t("usage.requestLogs")}
        >
          <thead>
            <tr className={usageTable.headRow}>
              <th className={usageTable.th}>{t("usage.time")}</th>
              <th className={usageTable.th}>{t("usage.app")}</th>
              <th className={usageTable.th}>{t("usage.provider")}</th>
              <th className={usageTable.th}>{t("usage.model")}</th>
              <th className={usageTable.th}>{t("usage.freshInput")}</th>
              <th className={usageTable.th}>{t("usage.outputTokens")}</th>
              <th className={usageTable.th}>{t("usage.cacheReadTokens")}</th>
              <th className={usageTable.th}>{t("usage.cost")}</th>
              <th
                className={cn(
                  usageTable.th,
                  "sticky end-[136px] z-20 w-[112px] bg-surface",
                )}
              >
                <span className="inline-flex items-center gap-0.5">
                  {t("usage.requestTiming")}
                  <HelpTip title={t("usage.firstTokenHelpTitle")} align="end">
                    {t("usage.firstTokenHelp")}
                  </HelpTip>
                </span>
              </th>
              <th
                className={cn(
                  usageTable.th,
                  "sticky end-0 z-20 w-[136px] bg-surface",
                )}
              >
                <span className="inline-flex items-center gap-0.5">
                  {t("usage.speed")}
                  <HelpTip title={t("usage.speedHelpTitle")} align="end">
                    {t("usage.requestSpeedHelp")}
                  </HelpTip>
                </span>
              </th>
            </tr>
          </thead>
          <tbody>
            {logs.length === 0 ? (
              <tr>
                <td colSpan={10} className={usageTable.empty}>
                  {t("usage.noData")}
                </td>
              </tr>
            ) : (
              logs.map(renderRow)
            )}
          </tbody>
        </table>
      </div>

      <section
        aria-label={t(
          isFullSummary
            ? "usage.requestSpeedSummary"
            : "usage.requestSpeedPageSummary",
        )}
        className="flex flex-wrap items-center gap-x-5 gap-y-1.5 border-b border-border bg-subtle px-2 py-2 text-caption tabular-nums"
      >
        <span className="inline-flex items-center gap-0.5 font-medium">
          {t(
            isFullSummary
              ? "usage.requestSpeedSummary"
              : "usage.requestSpeedPageSummary",
          )}
          <HelpTip title={t("usage.speedHelpTitle")}>
            {t("usage.requestSpeedSummaryHelp")}
          </HelpTip>
        </span>
        <span>
          {t("usage.afterFirstToken")}:
          <strong className="ms-1 font-medium">
            {generationSpeed == null
              ? "—"
              : `${speedSummary.estimatedGenerationRequestCount > 0 ? "≈" : ""}${generationSpeed} tok/s`}
          </strong>
        </span>
        <span>
          {t("usage.endToEnd")}:
          <strong className="ms-1 font-medium">
            {totalSpeed == null
              ? "—"
              : `${speedSummary.estimatedTimedRequestCount > 0 ? "≈" : ""}${totalSpeed} tok/s`}
          </strong>
        </span>
        <span className="text-fg-3">
          {t("usage.requestSpeedCoverage", {
            generation: speedSummary.generationRequestCount,
            timed: speedSummary.timedRequestCount,
            total: isFullSummary ? total : logs.length,
          })}
        </span>
      </section>

      <TablePagination
        page={page}
        totalPages={totalPages}
        total={total}
        onPageChange={setPage}
      />
    </div>
  );
}
