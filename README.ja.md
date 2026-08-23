# herdr-worktree-nav

**[English](./README.md)**

頭の中に収まらないほど大きくなった [herdr](https://herdr.dev) セッションを歩き回るためのプラグインです。

複数のリポジトリ・複数の worktree にまたがってエージェントを走らせていると、問いは「このエージェントは何をしているか」から「あれは *どこ* にあるか」に変わります。herdr-worktree-nav はそれに答えます。逆方向も同じで、GitHub 上にあるブランチを、キーボードから手を離さずに作業中の pane に変えられます。

ピッカーが 2 つ、それぞれキー 1 つで開き、`Tab` で行き来します。どちらも生きているセッションの上に popup として開き、描画は herdr 本体の session navigator と同じ方式です（herdr が描く枠、tree グリフ、herdr のテーマの accent 色）。別のプログラムではなく herdr の一部として読めます。

## Panes — 何がどこにあるか

開いている pane を、リポジトリと、チェックアウトされている worktree でグループ化して並べます。

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search panes                                                      13 panes │
│──────────────────────────────────────────────────────────────────────────────│
│ ◆ ● ShoMasegi/herdr-worktree-nav (2)                                         │
│   └── ● main                                  ~/Workspace/herdr-worktree-nav │
│ ◆    ├── ● claude                             w7:p2                          │
│      └── · shell                              w7:p3                          │
│                                                                              │
│   ○ ShoMasegi/harbour-backend (5)                                            │
│   ├── ○ feat/hbr-51-grant-table-privileges    ~/Workspace/harbour-backend    │
│   │  ├── ○ claude                             w1:p1                          │
│   │  └── · shell                              w1:p2                          │
│   └── · loop-review-fix-request  no pane      ~/.herdr/worktrees/harbour/…   │
│                                                                              │
│ ShoMasegi/herdr-worktree-nav · main · w7:p2 · working · ~/Workspace/herdr-w… │
│ ↵ jump  n new  ←→ repo  ⇥ branches  / search  b/w/i/d/a states  esc close    │
└──────────────────────────────────────────────────────────────────────────────┘
```

`●` 実行中 `○` 待機中 `◆` ブロック中 `·` エージェントなし。グリフの種類は herdr の設定に従います。gutter の `◆` は、いまセッションがどこにいるかを示します。一覧の下の行には、カーソル位置の行の詳しい文脈（checkout パスを含む）が出ます。

`Enter` でそこへ移動します。space をまたいでも tab をまたいでも、目的の pane に直接飛びます。pane が 1 つも無い worktree も一覧に出て、その行で `Enter` を押すと開きます。`←`/`→` で前／次のリポジトリの先頭に飛べます。`b`/`w`/`i`/`d` でエージェントの状態を 1 つに絞り込み、`a` で解除できます（navigator と同じキーです）。

## Branches — そのブランチで作業を始める

まずリポジトリを選びます。herdr が開いているものがすべて並び、呼び出した元のリポジトリには印が付き、最初からカーソルが当たっています。

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search repositories█                                          4 repositories│
│──────────────────────────────────────────────────────────────────────────────│
│ ◆ ShoMasegi/herdr-worktree-nav  1 worktree, 2 panes   ~/Workspace/herdr-work…│
│   ShoMasegi/harbour-backend     3 worktrees, 5 panes  ~/Workspace/harbour-ba…│
│   nightowl/harken               1 worktree, 1 pane    ~/Workspace/nightowl/h…│
│                                                                              │
│ ShoMasegi/herdr-worktree-nav · ~/Workspace/herdr-worktree-nav ───────────────│
│ ↵ branches  j/k move  / search  ⇥ panes  q close                             │
└──────────────────────────────────────────────────────────────────────────────┘
```

続いて、そのリポジトリのブランチを状態を問わず一覧します。

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search branches                                   ⇅ state ↓    24 branches │
│ me/app · ~/src/app ──────────────────────────────────────────────────────────│
│   ● feat/login    running      #123 Add the login screen (draft)             │
│   ○ fix/crash     checked out  latest work on fix/crash                      │
│   · main          local        latest work on main                           │
│   ↓ feat/search   remote                                                     │
│                                                                              │
│ feat/login · open in w2:p1 · ~/.herdr/worktrees/app/feat-login ──────────────│
│ ↵ choose  j/k move  / search  f fetch  i order  shift+i reverse  esc back    │
└──────────────────────────────────────────────────────────────────────────────┘
```

`/` で絞り込めます。まだ存在しない名前を打てば、それを作る候補が出ます。`i` で並び順（状態順・更新順・名前順）を巡回し、`Shift-I` で反転します。効いている並びは件数の隣に出ます。`f` でそのリポジトリを fetch します。続いて pane の行き先を選びます。

```
 here            split right     w1  app / agents
                 split down      ┌──────────────┬──────────────┐
 existing tab    w1  app / logs  │ ● claude     │ + feat/login │
                 w5  harken/…    │ w1:p1        │              │
 existing space  w1  app         ├──────────────┴──────────────┤
 new space       on its own      │ · shell                     │
                                 │ w1:p9                       │
                                 └─────────────────────────────┘
```

一覧の右には、カーソルがある行を選んだ場合にその tab がどうなるかが出ます。tab の実際のレイアウトに新しい pane を描き込んだものなので、行き先を想像せずに目で確かめられます。

`Enter` `Enter` が最速で、呼び出した pane の右に split されます。

その後ピッカーはその場に残り、今どの段階かを表示します（`fetching origin/feat/login`、`creating the worktree for feat/login`、pane の移動）。fetch と checkout は数秒かかる処理で、その数秒だけ空の箱が出ているとクラッシュしたようにしか見えないためです。失敗した場合は画面を保持し、git または herdr の言い分をそのまま表示します。

その次に何が起きるかはブランチの状態で変わります。ここがこのプラグインの要点です。

| ブランチの状態 | 起きること |
| --- | --- |
| すでに pane で開いている | そこへ移動します。すでにある作業を二重にチェックアウトしません |
| チェックアウト済みだが pane が無い | 指定した場所にその checkout が開きます |
| ローカルブランチ | そこから worktree を作ります |
| リモートにのみ存在（未 fetch） | fetch してから `origin/<branch>` を基点に作ります |
| どこにも無い | `HEAD` から作成し、そこから worktree を作ります |

worktree の作成場所は herdr の設定に従います（herdr の設定ファイルの `[worktrees] directory`、既定は `~/.herdr/worktrees`）。このプラグインが独自の場所を決めることはありません。

## インストール

```sh
herdr plugin install ShoMasegi/herdr-worktree-nav
```

herdr 0.7.4 以降と `git` が必要です。対応は macOS と Linux です。

インストール時はビルド済みバイナリをダウンロードしてチェックサムを検証します。対応するビルドが無い場合は `cargo build` にフォールバックするので、その場合は [Rust](https://rustup.rs) が必要です。

`gh` は任意です。インストール済みで認証が通っていれば、各ブランチに対応する open な pull request を表示します。それ以外は `gh` に依存しておらず、オフラインでもすべて動作します。

## キーの割り当て

herdr のプラグインは利用者のキーバインドを勝手に設定できません。`~/.config/herdr/config.toml` に次を追加してください。

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "herdr-worktree-nav.open-panes"
description = "list open panes"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-worktree-nav.open-branches"
description = "open a branch as a worktree"
```

`prefix+f` と `prefix+shift+b` は herdr 0.7.4 の既定と衝突しません。`prefix+g` は herdr 自身の `goto`、`prefix+shift+g` は `new_worktree` です。追加したら `herdr server reload-config` を実行します。

どちらのアクションも herdr のアクションメニューに出るほか、直接実行もできます。

```sh
herdr plugin action invoke herdr-worktree-nav.open-panes
```

## ドキュメント

- [インストール](docs/ja/installation.md)
- [使い方](docs/ja/usage.md) — すべてのキーと、その動作
- [設定](docs/ja/configuration.md)
- [アーキテクチャ](docs/ja/architecture.md) — 構成と、その理由
- [トラブルシューティング](docs/ja/troubleshooting.md)
- [設計判断の記録](docs/adr/) — 後から読んだ人が元に戻したくなるであろう選択の理由（英語）

## コントリビュート

[CONTRIBUTING.md](./CONTRIBUTING.md) を参照してください。開発規約は [CLAUDE.md](./CLAUDE.md) にあります。

## ライセンス

[MIT](./LICENSE)
