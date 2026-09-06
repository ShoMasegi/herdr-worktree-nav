# アーキテクチャ

[English](../en/architecture.md)

Rust のバイナリ 1 つです。herdr はこれを 3 通りの方法で起動し、ピッカー自身が 4 つ目を起動します。

```
キーバインド ──▶ herdr-worktree-nav action open-panes
                     │  HERDR_PLUGIN_CONTEXT_JSON を読む
                     │  呼び出し元の pane とリポジトリを env で渡す
                     ▼
                 plugin.pane.open ──▶ herdr-worktree-nav pane panes    ─▶ ピッカー
                                      herdr-worktree-nav pane branches  │
                                                                        │ Shift-D, y
診断 ──────────▶ herdr-worktree-nav dump                                ▼
                                      herdr-worktree-nav remove <repo> <path> <branch>
                                          setsid する。ピッカーを閉じても殺されないため
```

アクションはピッカーそのものではありません。アクションはプラグインディレクトリを作業ディレクトリとして起動され、利用者がどこにいたかを知りません。そのため、herdr から渡されたコンテキストを読み、正しい場所に pane を開くことだけが仕事です。

## レイヤー

```
src/
  main.rs      argv から 4 つのモードのいずれかへ
  app/         配線: herdr の各エントリポイントが何をするか
  ui/          描画とキー処理
  domain/      純粋ロジック — I/O を一切持たない
  port/        adapter より上のすべてが依存する trait
  adapter/     ソケットやプロセスに触れる唯一の場所
```

依存の向きは一方向です。`app` と `ui` は `domain` と `port` を使い、`domain` は標準ライブラリと port のデータ型以外を使わず、出荷されるバイナリで port を実装するのは `adapter` だけです。これは `scripts/check-invariants.sh` で強制し、CI で実行しています。

例外は `app::fakes` ひとつです。この層のテストが共有する記録用 port を集めた `#[cfg(test)]` モジュールで、バイナリには入りません。誰かの `mod tests` の中の非公開モジュールではなく名前付きモジュールにしてあるのは、2 つのモジュールが 1 本のログに対して表明できるようにするためです。`docs/adr/0010-closing-the-panes-first.md` の順序規則は 2 つの port にまたがるので、固定するには 2 本の列ではなく 1 本の列が要ります。

狙いは、重要な判断を herdr サーバーや git リポジトリ無しでテストできるようにすることです。ツリーの構築、ブランチが何であるかの判定、pane の行き先の計画は、すべてプレーンなデータ上の純粋関数です。

| モジュール | 答える問い |
| --- | --- |
| `domain::tree` | snapshot と git の回答から、repo → worktree → pane のツリーはどうなるか |
| `domain::rows` | この絞り込みで、どの行がどの順で見え、どれにカーソルが止まるか |
| `domain::resolve` | このブランチは *何* であり、選んだとき最初に何が必要か |
| `domain::order` | ブランチ一覧はどの順で、どちら向きに読むか |
| `domain::dest` | pane はどこに置けて、各選択はどの herdr 呼び出しになるか |
| `domain::preview` | pane が着地した後、行き先の tab はどう見えるか |
| `domain::progress` | ブランチを開く処理は今どの段階で、まだ中断できるか |
| `domain::removal` | どのチェックアウトを削除するのか、そして終わった削除は何を、誰に向かって言うのか |
| `domain::sweep` | 一括削除はどのチェックアウトを、どんな理由で提示してよいのか、そしてどれは判断できなかったのか |
| `domain::chrome` | herdr はどの accent と状態グリフに設定されているか |

## herdr との通信

`herdr` CLI ではなく `HERDR_SOCKET_PATH` のソケット経由です。決め手は `pane.focus` で、これは CLI では表現できません（[ADR 0002](../adr/0002-socket-transport.md)）。

プロトコルは 1 リクエスト 1 コネクションです。接続し、JSON を 1 行書き、1 行読むと、サーバー側が閉じます。そのため `SocketHerdr` にはコネクションプールが無く、接続の寿命管理を誤る余地もありません。

ワイヤー型は意図的に寛容です。省略可能なフィールドはすべて既定値を持ち、未知のフィールドは無視します。フィールドが増えた新しい herdr でも、起動に失敗せずパースできます。

この API の速く安全な側に留まるための決まりが 4 つあります。

- **`$HERDR_BIN_PATH` 経由で呼ぶ**（無ければ `PATH` 上の `herdr`）。パスを直書きしないこと。インストール方法によってバイナリの場所は変わります
- **`herdr api snapshot` を 1 回。細かい呼び出しを並べない。** workspaces・tabs・panes・agents・layouts がまとめて返り、1 回の読み取りなので互いに整合しています
- **herdr が既に算出したものを使う。** worktree に紐づく workspace なら `WorkspaceInfo.worktree` が `repo_key` / `repo_root` / `checkout_path` を持ち、`WorktreeInfo.open_workspace_id` が worktree が開いているかを教えます。`git` を叩くのは、herdr が worktree として把握していない pane のためだけです
- **`herdr worktree create` は必ず新しい workspace・tab・root pane を作る。** 既存の tab に入れる選択肢はありません。別の場所に置きたければ、作ってから root pane を移動し、空になった workspace を閉じることになります。`git worktree add` を直接呼ぶより、なぜこちらが良いかは [ADR 0001](../adr/0001-delegate-worktree-creation.md) を参照してください

git の情報は作業ディレクトリ単位で解決してキャッシュします。複数の pane が同じ cwd を共有することが多く、pane ごとに `git` を起動するピッカーは体感で分かるほど遅くなります。

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
                    リポジトリごとに for-each-ref ─▶ ahead/behind、gone、
                                ▼                    どの checkout がどのブランチを持つか
                          domain::tree::build
                                │
                                ▼
                    checkout ごとに git --no-optional-locks status --porcelain
                    （8 並列、起動したビューより長生きするスレッドの上で）
                                │
                        Shift-S ▼
                    リポジトリごとに gh pr list --state closed
                    （1 スレッドずつ、ピッカーが開いている間ずっと保持）
```

即座に開くための工夫が 2 つあります。作業ディレクトリの解決は pane ごとではなく重複を除いた cwd ごとに 1 回だけ行います（複数の pane が同じ cwd を共有することが多いためです）。また、herdr が既にその workspace を worktree として把握している場合は git を実行せずその答えを使います。ただし、pane がその checkout 配下に留まっている場合に限ります。pane はいつでも隣のリポジトリへ `cd` できるためです。

pane と worktree の対応付けは checkout パスで行い、`open_workspace_id` では行いません。pane を別の場所へ移した worktree は、実際に pane が動いていても `open_workspace_id: None` を返すためです。

各 checkout が今どういう状態かは、2 つの速度で届きます。この分け方自体が設計です。ahead / behind と `gone` は、どのみち実行される `for-each-ref` のフィールドです——ブランチごとの `rev-list --count` ではなくリポジトリごとに 1 プロセス——なので、最初のフレームで画面に出ます。作業ツリーが汚れているかはそのツリーを歩くことであり、しかも checkout ごとに 1 回必要なので、最初のフレームの裏で訊き、答えが届いた行から埋めます。まだ答えの無い checkout は、間違った印ではなく印なしで描きます。答えはピッカーが開いている間ずっと保持します。ビュー切り替え側が所有していて panes ビューが所有していないのは、そのためです。`✱` の入る場所は最初のフレームから確保してあるので、答えが届いても隣のパスは動きません。`r` は答えを全部捨てて訊き直し、その前に発した問いへの答えは、新しい答えと取り違えずに捨てます。

sweep は同じ形で 3 つ目の問いを立てます。各リポジトリの pull request がどうなったかを `gh` に訊くのはリポジトリごとに 1 プロセスで、ピッカーを開いたときではなく `Shift-S` で始めます——2 つある `gh` の呼び出しのうち重いほうで、しかも sweep をしないセッションが大半だからです。答えはループが届いた順に取り込むので、遅いネットワークで入った sweep は「何も見つからなかった」に見える代わりに `asking gh…` と言います。作業ツリーの答えと同じ理由でビュー切り替え側が所有し、保持します。マージ済みの pull request がマージ前に戻ることは無いので、sweep を抜けて入り直しても map の参照 1 回で済みます。入り際に訊き直すのは `gh` が答えられなかったリポジトリだけで、`r` は答えを全部捨てます。`gh` が答えられなかったリポジトリはプロンプト行に名前入りで出て、その行は何も言わない代わりに `PR unknown` と言います——[ADR 0011](../adr/0011-what-may-be-swept.md)。

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

これらはすべてワーカースレッドで実行され、その間ピッカーは画面に残って今どの段階かを表示します。fetch と checkout は数秒かかる処理で、その間だけ真っ白になるピッカーはハングしたものと区別がつかないためです（[ADR 0007](../adr/0007-stay-up-while-working.md)）。`HerdrPort` が `Sync` なのも同じ理由です。

`worktree.create` は必ず workspace を丸ごと作ります。既存 tab に pane を作らせる方法はありません。そのため「新しい space」以外の行き先はすべて、作成してから移動することで実現しています。空になった tab と workspace は herdr 自身が閉じ、checkout はそのまま残ります。これが後始末を不要にしています（[ADR 0001](../adr/0001-delegate-worktree-creation.md)）。

## checkout を削除する

ここに書かれている他のすべてはピッカーのプロセスの中で起きます。削除だけは違います。`git worktree remove` は working tree 全体を歩いてからでないと消せず、`y` と答えた後にユーザーが自然に取る行動はピッカーを閉じることだからです。

```
Shift-D, y ─▶ setsid herdr-worktree-nav remove …  ─┬─▶ git worktree remove
                   │                               └─▶ notification.show   常に
                   │ stdout に 1 行
                   ▼
             ピッカー（まだ開いていれば）:
               行に deleting ⠻、断られた理由はプロンプト行に
```

報告するのは子プロセスで、ピッカーはそれを飾るだけです。パイプに書いた 1 行が読まれたかどうかは、ループにも子にも分かりません——利用者は Branches ビューにいるかもしれず、既に閉じているかもしれない——ので、通知は無条件に出し、成功時にピッカーは何も足しません。`setsid` は本質的な部分です。herdr は閉じた pane のプロセスグループを殺すためです（[ADR 0014](../adr/0014-removing-outlives-the-picker.md)）。

## herdr に見た目を揃える

ピッカーは herdr 本体の session navigator と同じ方式で描画しています（`src/ui/navigator.rs` を参照して再現）。パネル、検索行、tree グリフ、gutter、meta 列、詳細行、キーヒントが対象です。対応付けは `ui::theme` にあり、accent とグリフ種別は herdr の設定から読みます（API が palette を公開していないためです）。何を写して何を写していないかは [ADR 0004](../adr/0004-navigator-appearance.md) を参照してください。

## テスト

`domain` はテストを先に書きます。落ちるテストを書いてから実装します。plain なデータを受け取って plain なデータを返すだけの層で、言い訳の余地がなく、間違えると困る判断はここに集まっているためです。

| レイヤー | 方法 |
| --- | --- |
| `domain` | Fake の port を注入した単体テスト。すべてのブランチ状態とすべての行き先を網羅 |
| `ui` の状態 | キー処理は状態 → アクションの純粋な写像なので、キーマップを直接テスト |
| `ui` の描画 | `TestBackend` + `insta` による描画バッファのスナップショット |
| `adapter` の git | `tempfile::TempDir` に実リポジトリを作成 |
| `adapter` の gh | CI では `gh` を一度も起動しないため、各呼び出しを「組み立てるコマンド」と「読む答え」に分割し、両方をプロセス無しでテストする — 不正な引数列が緑のスイートを素通りして2度出荷されたため。テストが届かないのは `.output()` とその周りのリダイレクトだけ: `Command` は `stderr` の getter を持たないので、`null` に送っても（refusal のたびに gh 自身の言葉が失われるのに）何も落ちない。ここはテストが `PATH` に置いた `gh` が要る |
| `adapter` の herdr | CI ではテスト不可（サーバーが無い）。手動確認手順は[トラブルシューティング](troubleshooting.md)を参照 |
