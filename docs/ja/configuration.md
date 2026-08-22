# 設定

[English](../en/configuration.md)

herdr-worktree-nav は独自の設定ファイルを持ちません。参照するのは herdr の設定か、リポジトリの状態だけです。これは意図的な設計です。worktree の置き場所について 2 つのツールが食い違うことのほうが、ツマミが少ないことより有害だからです。

## キーバインド

プラグインは利用者のキーバインドを設定できません。`~/.config/herdr/config.toml` に追加してください。

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-worktree-nav.open-panes"
description = "list open panes"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-worktree-nav.open-branches"
description = "open a branch as a worktree"
```

`herdr server reload-config` で反映します。

どちらのアクションもキー割り当て無しで herdr のアクションメニューから使えるほか、直接実行もできます。

```sh
herdr plugin action invoke herdr-worktree-nav.open-panes
herdr plugin action invoke herdr-worktree-nav.open-branches
```

ピッカーは popup として開きます。herdr は開いている popup に対して、自身のキーバインドを解釈する前にすべてのキーを送るため、ピッカーが出ている間は同じキーを押しても発火しません。まず `Esc` で閉じてください。

## worktree の作成場所

このプラグインの設定ではなく、herdr の設定です。

```toml
[worktrees]
directory = "~/.herdr/worktrees"
```

checkout は `<directory>/<repo>/<branch-slug>` に置かれます。このプラグインは herdr に作成を依頼するだけで、このパスを自分で計算することはありません。理由は [ADR 0001](../adr/0001-delegate-worktree-creation.md) を参照してください。

プロジェクトの隣に並べたい場合は、ディレクトリをそちらに向けます。

```toml
[worktrees]
directory = "~/Workspace/worktrees"
```

## 見た目

ピッカーは herdr の session navigator と同じ方式で描画されます。見た目に影響する herdr 側の設定が 2 つあり、どちらもこのプラグインの設定ではなく、herdr の設定をそのまま読んでいます。

```toml
[theme]
name = "catppuccin"        # 枠・選択行・リポジトリ行に使う accent

[ui]
status_indicators = "dots" # または "symbols": ● ● ● ○ ·  と  × ◐ ✓ ○ ·
```

accent の解決順は herdr と同じです。明示的な `[theme.custom] accent` が最優先、次に既定の `cyan` から変更された `[ui] accent`、最後にテーマ自身の accent です。このプラグインが知らないテーマ名の場合は推測せず cyan にフォールバックするので、herdr が新しいテーマを追加しても動作します。

それ以外は端末自身の 16 色を使うため、ピッカーは端末のテーマに逆らわず従います。herdr の palette はプラグインからは取得できず（socket API がテーマを一切公開していません）、完全一致は選択肢にありません。[ADR 0004](../adr/0004-navigator-appearance.md) を参照してください。

解決結果は `herdr-worktree-nav dump` で確認するのが最も手軽です。

## リモート

リモートブランチの取得元、および未 fetch ブランチの fetch 元は `origin` です。v1 では変更できません。`origin` が無いリポジトリでも動作します。その場合、ブランチ一覧はローカルにあるものだけになり、`reading the remote…` の表示は消えます。

## Pull request

`gh` が `PATH` にあり、そのリポジトリに対して認証が通っていれば、open な pull request が対応ブランチに表示され、番号やタイトルで検索できます。無い場合も他の挙動は変わりません。このレイヤーがピッカーを失敗させることはありません（[ADR 0003](../adr/0003-git-first-gh-optional.md)）。

ピッカーから見えている内容を確認するには次を実行します。

```sh
gh auth status
gh pr list --json number,title,headRefName,isDraft
```

## 環境変数

以下は herdr が設定します。利用者が設定するものではありません。

| 変数 | 用途 |
| --- | --- |
| `HERDR_SOCKET_PATH` | API ソケット。無い場合は説明を出して終了します。 |
| `HERDR_PLUGIN_CONTEXT_JSON` | アクションがどの pane・どのリポジトリから呼ばれたか |
| `HERDR_PLUGIN_ROOT` | pane エントリポイントからバイナリを特定するため |
| `HERDR_PLUGIN_CONFIG_DIR` | herdr 本体の `config.toml` の場所を特定し、上記 2 設定を読むために使います |

アクションは、pane プロセスが自力では知り得ない次の 2 つを、開く pane に渡します。

| 変数 | 意味 |
| --- | --- |
| `GH_NAV_FROM_PANE` | ピッカーを呼び出した pane |
| `GH_NAV_REPO_ROOT` | 呼び出し元のリポジトリ（herdr が既に把握していた場合） |
