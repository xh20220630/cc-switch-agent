import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { localInvoke } from "@/lib/runtime/invoke";
import type {
  DiscoveredSshTarget,
  RemotePlatform,
  RemoteTargetConfig,
  RuntimeSnapshot,
} from "@/lib/runtime/types";

export const remoteApi = {
  async discoverTargets(): Promise<DiscoveredSshTarget[]> {
    // 服务器发现固定走本机 Tauri 边界；远端运行时不能读取操作者的 ~/.ssh/config。
    return await localInvoke("remote_discover_ssh_targets");
  },

  async listTargets(): Promise<RemoteTargetConfig[]> {
    return await localInvoke("remote_list_targets");
  },

  async upsertTarget(target: RemoteTargetConfig): Promise<boolean> {
    return await localInvoke("remote_upsert_target", { target });
  },

  async testTarget(target: RemoteTargetConfig): Promise<RemotePlatform> {
    // 连接元数据始终由本机后端处理；测试不能穿过当前远程运行时再次转发。
    return await localInvoke("remote_test_target", { target });
  },

  async deleteTarget(targetId: string): Promise<boolean> {
    return await localInvoke("remote_delete_target", { targetId });
  },

  async getSnapshot(): Promise<RuntimeSnapshot> {
    return await localInvoke("remote_get_runtime_snapshot");
  },

  async setActiveTarget(
    targetId?: string,
    password?: string,
  ): Promise<RuntimeSnapshot> {
    return await localInvoke("remote_set_active_target", {
      targetId: targetId ?? null,
      password: password ?? null,
    });
  },

  async saveTargetPassword(
    targetId: string,
    password: string,
  ): Promise<boolean> {
    return await localInvoke("remote_save_target_password", {
      targetId,
      password,
    });
  },

  async deleteTargetPassword(targetId: string): Promise<boolean> {
    return await localInvoke("remote_delete_target_password", { targetId });
  },

  async hasTargetPassword(targetId: string): Promise<boolean> {
    return await localInvoke("remote_has_target_password", { targetId });
  },

  async trustTargetHost(target: RemoteTargetConfig): Promise<string[]> {
    // 类似 XShell 首次连接：把服务器公钥写入 known_hosts，返回密钥指纹。
    return await localInvoke("remote_trust_target_host", { target });
  },

  async getHostKeyFingerprints(target: RemoteTargetConfig): Promise<string[]> {
    // 仅读取服务器公钥指纹，用户确认前不写入 known_hosts。
    return await localInvoke("remote_get_host_key_fingerprints", { target });
  },

  async onStatus(
    handler: (snapshot: RuntimeSnapshot) => void,
  ): Promise<UnlistenFn> {
    return await listen<RuntimeSnapshot>("remote-runtime-status", (event) => {
      handler(event.payload);
    });
  },
};
