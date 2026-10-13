// 供应商和模式查询共用带主机的键，写入后的失效才能刷新当前目标的列表。
import { providerKeys } from "@/lib/query/queries";
import { useQuery, type QueryClient } from "@tanstack/react-query";
import { piApi } from "@/lib/api/pi";

export const piKeys = {
  all: ["pi"] as const,
  currentState: ["pi", "currentState"] as const,
  sessionDiscovery: ["pi", "sessionDiscovery"] as const,
};

export const invalidatePiProviderCaches = async (queryClient: QueryClient) => {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: piKeys.currentState }),
    queryClient.invalidateQueries({ queryKey: providerKeys.byApp("pi") }),
  ]);
};

export const invalidatePiDirectoryCaches = async (queryClient: QueryClient) => {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: piKeys.all }),
    queryClient.invalidateQueries({ queryKey: providerKeys.byApp("pi") }),
    queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
    queryClient.invalidateQueries({ queryKey: ["sessions"] }),
  ]);
};

export function usePiCurrentState(enabled = true) {
  return useQuery({
    queryKey: piKeys.currentState,
    queryFn: () => piApi.getCurrentState(),
    enabled,
  });
}
