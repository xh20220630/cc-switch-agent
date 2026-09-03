<div align="center">

# CC Switch — リモートサーバー版

### [farion1231/cc-switch](https://github.com/farion1231/cc-switch) の独立フォーク

[![Version](https://img.shields.io/github/v/release/xh20220630/cc-switch-agent?color=blue&label=version)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey.svg)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

[English](README.md) | [中文](README_ZH.md) | 日本語 | [Deutsch](README_DE.md)

</div>

---

## これは何か

このリポジトリは [CC Switch](https://github.com/farion1231/cc-switch)（MIT）の独立フォークです。上流は Tauri 2 製のデスクトップアプリで、Claude Code、Claude Desktop、Codex、Gemini CLI、Grok Build、OpenCode、OpenClaw、Hermes などのツールの複数プロバイダー API 設定を一元管理します。ワンクリック切り替え、MCP / Prompts / Skills 管理、プロキシとフェイルオーバー、利用量トラッキング、クラウド同期などを備えます。

**上流の完全な機能とドキュメントは上流リポジトリを直接参照してください:**

- プロジェクトホームとドキュメント: <https://github.com/farion1231/cc-switch>
- ユーザーマニュアル: <https://github.com/farion1231/cc-switch/tree/main/docs/user-manual>
- チェンジログ: <https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md>

本フォークは **上流の機能をすべて維持したうえで「リモートサーバーモード」を追加**します。SSH でリモート Linux サーバーに接続し、リモート側で一時エージェントを実行して、リモートの `~/.cc-switch` データベースを直接読み書きします。リモート上で動く Claude / Gemini / OpenCode / Grok / Codex のセッション利用量をデスクトップに取り込んで表示し、ローカルとリモート間でプロバイダーの同期も行えます。

---

## このフォークが追加するもの

### リモートサーバーモード（Remote Runtime）

多くのユーザーは Claude Code / Codex / Gemini を**リモート Linux サーバー**（クラウドホスト、コンテナ、開発機）で実行しており、デスクトップはローカル設定だけを管理しています。本フォークはデスクトップからリモートへ直結し、「計算コアを運搬」します:

- **SSH ターゲット管理** — `~/.ssh/config` からターゲットを自動発見、または手動追加。接続テスト・編集・削除
- **一時エージェント、接続とともに出入り** — デスクトップに musl 静的リンクの Linux エージェント（x86_64 / aarch64 の 2 アーキテクチャ）を同梱。接続時に scp で `/tmp` へアップロードし、`--stdio` で実行、終了時に自身を自動削除（trap クリーンアップ + デスクトップ側フォールバック）
- **リモート DB を直接読み書き** — エージェントはリモートユーザー自身の `~/.cc-switch/cc-switch.db` を開き、ローカルと完全に同じ挙動（同じ Core インポーター）—「コピーの取得」ではありません
- **リモート利用量同期** — 接続成功時に 1 回、Usage ページを開いたときにさらに 1 回自動同期、あとはいつでも手動同期。Claude / Gemini / OpenCode / Grok / Codex の 5 ソースのセッションログをリモート DB に取り込み
- **プロバイダー同期** — ローカルとリモート間でプロバイダーを同期。検索・重複スキップ・リトライ付きのダイアログ
- **ランタイムスイッチャー** — トップバーで「ローカル / リモートターゲット」を切り替え、リアルタイムステータス表示（connecting / online / offline / reconnecting / incompatible）

```
┌─ デスクトップ (Windows / Linux) ─┐     SSH      ┌─ リモートサーバー (Linux) ───┐
│ Tauri フロントエンド UI         │   scp/ssh   │ 一時エージェント (cc-switch-agent)│
│  appInvoke ─► remote_invoke    │ ──────────► │  │ リモート ~/.cc-switch/ を開く │
│  RemoteRuntimeState            │ ◄────────── │  │  cc-switch.db                 │
│  埋め込みエージェントバイト     │             │  │ セッションログ: ~/.claude …  │
└────────────────────────────────┘             │  │ ~/.gemini ~/.grok ~/.codex    │
                                               └──────────────────────────────────┘
```

### セキュリティ設計

| リスク | 対策 |
|---|---|
| コマンドインジェクション | リモートコマンドは制御されたテンプレート（16 進パス / 10 進長 / SHA-256）から組み立てられ、ユーザー入力はシェルに到達しない。SSH 引数は `OsString` 配列で直接渡し、ローカルシェルを経由しない |
| 権限外コマンド | 中央の能力ホワイトリスト（`cc-switch-protocol`、27 コマンド）をハンドシェイク時に返し、デスクトップ側で `require()` が検証。未登録のローカルコマンドはリモート実行を拒否 |
| クロストーク（ターゲット切替後の旧応答） | generation トークンの二重検証 + フロントエンドのクエリキャッシュをスコープ分離。切り替え中の遅延応答は拒否 |
| 認証情報の漏えい | パスワードは今回の接続のみのメモリ入力で、`remote-targets.json` には決して書かない。Windows では DPAPI（CryptProtectData）で暗号化保存が可能、SSH_ASKPASS で TTY なし認証を補助 |
| ホストキー | 初回接続時に `ssh-keyscan` でフィンガープリントを表示し、ユーザー確認後にのみ `known_hosts` へ書き込む（XShell 風） |
| リモートの残骸 | エージェントは trap で自己削除 + デスクトップ側の Drop 時 SSH クリーンアップ |
| stdout ログ汚染 | エージェントは診断を stderr のみに出力。プロトコルフレームは magic マーカーで混線を検出 |
| タイムアウトでのハング | コマンドごとにホワイトリスト化されたタイムアウト（30s〜300s）。タイムアウト時は Cancel を送信（requestId + operationId で誤キャンセルを防止） |

### リモートの対応範囲

- **リモートサーバー**: Linux のみ（`uname -s` プリフライト）、x86_64 または aarch64
- **デスクトップ**: Windows（x86_64 / ARM64）、Linux（x86_64 / ARM64）
- 読み取るリモートデータ: リモート `$HOME` 配下のセッションファイル — `~/.claude`、`~/.gemini`、`~/.local/share/opencode`、`~/.grok`、`~/.codex` — および `~/.cc-switch/` データベース。デスクトップのローカルファイルはアップロードも読み取りもされません

### 上流との差異

| 項目 | 上流 | 本フォーク |
|---|---|---|
| リモートサーバーモード | なし | ✅ 追加（本リポジトリの核） |
| プロバイダー同期 | なし | ✅ 追加 |
| ビルドプラットフォーム | Windows / macOS / Linux | Windows x64/ARM64 + Linux x64/ARM64 |
| macOS 署名 / 公証 | Apple Developer ID + notarization | なし（フォークは Apple 証明書を持たず、macOS ビルドなし） |
| 自動更新の署名 / エンドポイント | 上流の鍵とリポジトリ | フォーク自身の署名鍵と本リポジトリ |
| バージョン | 上流に追随する 3.x | 0.0.1 の独立リリース |

---

## インストールとダウンロード

最新版は [Releases](https://github.com/xh20220630/cc-switch-agent/releases) から:

| プラットフォーム | ファイル |
|---|---|
| Windows (x86_64) | `CC-Switch-{version}-Windows.msi`（インストーラー）/ `Windows-Portable.zip`（ポータブル） |
| Windows (ARM64) | `CC-Switch-{version}-Windows-arm64.msi` / `Windows-arm64-Portable.zip` |
| Linux (x86_64) | `CC-Switch-{version}-Linux-x86_64.AppImage` / `.deb` / `.rpm` |
| Linux (ARM64) | `CC-Switch-{version}-Linux-arm64.AppImage` / `.deb` / `.rpm` |

> 本フォークは macOS ビルドを提供しません（Apple 署名証明書がないため）。macOS ユーザーは上流のリリースを使用してください。
> `.tar.gz` は Tauri updater 専用で、手動ダウンロードは不要です。

## クイックスタート: リモートサーバーへ接続

1. **リモートを準備** — Linux サーバー（x86_64 / aarch64）、SSH 有効、リモートユーザーの `~/.cc-switch` とセッションディレクトリへアクセス可能
2. **ターゲットを追加** — 設定 → Remote Targets。`~/.ssh/config` を自動スキャン、またはホスト / ユーザー / ポートを手動入力
3. **接続** — 初回接続時、`ssh-keyscan` でサーバーの公開鍵フィンガープリントを表示。確認後にのみ `known_hosts` へ書き込み。パスワードは今回の接続のみ（Windows では暗号化保存が任意）
4. **ランタイムを切替** — トップバーのランタイムスイッチャーでターゲットを選択。接続成功時にリモート利用量を 1 回同期、Usage ページを開いたときにさらに 1 回、以降は「セッションを同期」でいつでも同期
5. **プロバイダー同期** — プロバイダー同期ダイアログでローカルのプロバイダーをリモートへプッシュ（またはリモートから取得）。検索と重複スキップに対応

> リモートに 60 秒の常駐ポーリングはありません。エージェントは接続とともに生まれる一時プロセスなので、同期タイミングは「接続成功 + ページ表示 + 手動」です。

## FAQ

<details>
<summary><strong>接続後、リモートの利用量データはいつ表示されますか？</strong></summary>

接続成功の瞬間にバックエンドが 1 回同期し、Usage ページを開いたときにさらに 1 回、以降は「セッションを同期」ボタンでいつでも同期できます。リモートに 60 秒の常駐ポーリングはありません。

</details>

<details>
<summary><strong>リモートがローカルのファイルを読み取ることはありますか？</strong></summary>

ありません。リモートエージェントはリモート `$HOME` 配下のセッションファイルと `~/.cc-switch/` データベースのみを開きます。デスクトップのローカルファイルはアップロードも読み取りもされません。

</details>

<details>
<summary><strong>パスワードはどこに保存されますか？</strong></summary>

接続パスワードはその接続のみのメモリ入力で、`remote-targets.json` には決して書き込まれません。Windows では任意で DPAPI 暗号化して `~/.cc-switch/remote-credentials.json` に保存できます。非 Windows プラットフォームではまだ資格情報の保存に対応していません。

</details>

<details>
<summary><strong>なぜ macOS ビルドがないのですか？</strong></summary>

このフォークは Apple Developer ID 証明書を持たず、macOS アプリに署名 / 公証ができません。macOS をお使いの場合は上流を使用してください: <https://github.com/farion1231/cc-switch/releases>

</details>

## ドキュメント

- **リモート同期の実装解説**（中国語）: `docs/remote-sync-architecture-zh.md`
- **ローカル / リモートのリクエスト分離設計**（中国語）: `docs/remote-route-isolation-design.md`
- 上流機能のマニュアル・FAQ・チェンジログは上流リポジトリにあります: <https://github.com/farion1231/cc-switch>

## 開発

### 必要環境

- Node.js 18+、pnpm 8+
- Rust 1.85+、Tauri CLI 2.8+

### コマンド

```bash
pnpm install        # 依存関係のインストール
pnpm dev            # 開発モード（ホットリロード）
pnpm typecheck      # フロントエンドの型チェック
pnpm test:unit      # フロントエンド単体テスト
cd src-tauri
cargo test          # バックエンドテスト
pnpm tauri build    # アプリのビルド
```

> リモートエージェントは musl 静的リンクの Linux バイナリ（x86_64 / aarch64）で、`src-tauri/build.rs` によりデスクトップバンドルへ埋め込まれます。GitHub Actions のリリースワークフローがクロスコンパイルとパッケージングを行います。

## ライセンス

MIT © Jason Young（上流）— 本フォークは [MIT License](LICENSE) のもとで配布されます。
