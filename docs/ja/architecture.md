# アーキテクチャ

[English](../en/architecture.md)

Rust のバイナリ 1 つです。herdr はこれを 3 通りの方法で起動します。

```
キーバインド ──▶ herdr-gh-nav action open-panes
                     │  HERDR_PLUGIN_CONTEXT_JSON を読む
                     │  呼び出し元の pane とリポジトリを env で渡す
                     ▼
                 plugin.pane.open ──▶ herdr-gh-nav pane panes    ─▶ ピッカー
                                      herdr-gh-nav pane branches

診断 ──────────▶ herdr-gh-nav dump
```

アクションはピッカーそのものではありません。アクションはプラグインディレクトリを作業ディレクトリとして起動され、利用者がどこにいたかを知りません。そのため、herdr から渡されたコンテキストを読み、正しい場所に pane を開くことだけが仕事です。

## レイヤー

```
src/
  main.rs      argv から 3 つのモードのいずれかへ
  app/         配線: herdr の各エントリポイントが何をするか
  ui/          描画とキー処理
  domain/      純粋ロジック — I/O を一切持たない
  port/        adapter より上のすべてが依存する trait
  adapter/     ソケットやプロセスに触れる唯一の場所
```

依存の向きは一方向です。`app` と `ui` は `domain` と `port` を使い、`domain` は標準ライブラリと port のデータ型以外を使わず、port を実装するのは `adapter` だけです。これは `scripts/check-invariants.sh` で強制し、CI で実行しています。

狙いは、重要な判断を herdr サーバーや git リポジトリ無しでテストできるようにすることです。ツリーの構築、ブランチが何であるかの判定、pane の行き先の計画は、すべてプレーンなデータ上の純粋関数です。

| モジュール | 答える問い |
| --- | --- |
| `domain::tree` | snapshot と git の回答から、repo → worktree → pane のツリーはどうなるか |
| `domain::rows` | この絞り込みとこの折りたたみ状態で、どの行がどの順で見えるか |
| `domain::resolve` | このブランチは *何* であり、選んだとき最初に何が必要か |
| `domain::order` | ブランチ一覧はどの順で、どちら向きに読むか |
| `domain::dest` | pane はどこに置けて、各選択はどの herdr 呼び出しになるか |
| `domain::preview` | pane が着地した後、行き先の tab はどう見えるか |
| `domain::chrome` | herdr はどの accent と状態グリフに設定されているか |

## herdr との通信

`herdr` CLI ではなく `HERDR_SOCKET_PATH` のソケット経由です。決め手は `pane.focus` で、これは CLI では表現できません（[ADR 0002](../adr/0002-socket-transport.md)）。

プロトコルは 1 リクエスト 1 コネクションです。接続し、JSON を 1 行書き、1 行読むと、サーバー側が閉じます。そのため `SocketHerdr` にはコネクションプールが無く、接続の寿命管理を誤る余地もありません。

ワイヤー型は意図的に寛容です。省略可能なフィールドはすべて既定値を持ち、未知のフィールドは無視します。フィールドが増えた新しい herdr でも、起動に失敗せずパースできます。

## Panes ビューの構築

```
herdr api snapshot ─┬─▶ workspaces（一部は .worktree を持つ: repo_key, repo_root, checkout_path）
                    ├─▶ tabs
                    └─▶ panes（cwd, agent, agent_status — git 情報は無い）
                                │
                    ┌───────────┴────────────┐
                    ▼                        ▼
      workspace が worktree だと         cwd ごとに git rev-parse
      herdr が既に知っているか            （8 並列）
                    └───────────┬────────────┘
                                ▼
                    リポジトリごとに worktree.list
                                ▼
                          domain::tree::build
```

即座に開くための工夫が 2 つあります。作業ディレクトリの解決は pane ごとではなく重複を除いた cwd ごとに 1 回だけ行います（複数の pane が同じ cwd を共有することが多いためです）。また、herdr が既にその workspace を worktree として把握している場合は git を実行せずその答えを使います。ただし、pane がその checkout 配下に留まっている場合に限ります。pane はいつでも隣のリポジトリへ `cd` できるためです。

pane と worktree の対応付けは checkout パスで行い、`open_workspace_id` では行いません。pane を別の場所へ移した worktree は、実際に pane が動いていても `open_workspace_id: None` を返すためです。

## ブランチを開く

まずリポジトリを選びます。Branches ビューは上のツリーにあるリポジトリをすべて並べ、ピッカーを呼び出した元に印を付け、そのリポジトリのブランチだけは最初のフレームの前に読みます。残りは選ばれた時点で読み、ピッカーを開いている間はキャッシュします。リポジトリ間を行き来しても git は再実行されません。ブランチをどの順で読むかは `domain::order` が決めます（[ADR 0006](../adr/0006-repository-step-and-branch-order.md)）。

```
BranchPlan             その後、どの plan でも共通:
──────────
Focus      ─▶ pane.focus                      placement_for(destination)
Open       ─▶ worktree.open  ─┐                   ├─ Some ─▶ pane.move ─▶ focus
Create     ─▶ worktree.create ┼─▶ root_pane ──────┤
FetchThen… ─▶ fetch, create  ─┘                   └─ None ─▶ pane.focus
```

`worktree.create` は必ず workspace を丸ごと作ります。既存 tab に pane を作らせる方法はありません。そのため「新しい space」以外の行き先はすべて、作成してから移動することで実現しています。空になった tab と workspace は herdr 自身が閉じ、checkout はそのまま残ります。これが後始末を不要にしています（[ADR 0001](../adr/0001-delegate-worktree-creation.md)）。

## herdr に見た目を揃える

ピッカーは herdr 本体の session navigator と同じ方式で描画しています（`src/ui/navigator.rs` を参照して再現）。パネル、検索行、tree グリフ、gutter、meta 列、詳細行、キーヒントが対象です。対応付けは `ui::theme` にあり、accent とグリフ種別は herdr の設定から読みます（API が palette を公開していないためです）。何を写して何を写していないかは [ADR 0004](../adr/0004-navigator-appearance.md) を参照してください。

## テスト

| レイヤー | 方法 |
| --- | --- |
| `domain` | Fake の port を注入した単体テスト。すべてのブランチ状態とすべての行き先を網羅 |
| `ui` の状態 | キー処理は状態 → アクションの純粋な写像なので、キーマップを直接テスト |
| `ui` の描画 | `TestBackend` + `insta` による描画バッファのスナップショット |
| `adapter` の git | `tempfile::TempDir` に実リポジトリを作成 |
| `adapter` の herdr | CI ではテスト不可（サーバーが無い）。手動確認手順は[トラブルシューティング](troubleshooting.md)を参照 |
