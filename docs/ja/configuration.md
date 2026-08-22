# 設定

[English](../en/configuration.md)

herdr-gh-nav は独自の設定ファイルを持ちません。参照するのは herdr の設定か、リポジトリの状態だけです。これは意図的な設計です。worktree の置き場所について 2 つのツールが食い違うことのほうが、ツマミが少ないことより有害だからです。

## キーバインド

プラグインは利用者のキーバインドを設定できません。`~/.config/herdr/config.toml` に追加してください。

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-gh-nav.open-panes"
description = "list open panes"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-gh-nav.open-branches"
description = "open a branch as a worktree"
```

`herdr server reload-config` で反映します。

どちらのアクションもキー割り当て無しで herdr のアクションメニューから使えるほか、直接実行もできます。

```sh
herdr plugin action invoke herdr-gh-nav.open-panes
herdr plugin action invoke herdr-gh-nav.open-branches
```

ピッカーが開いている状態でもう一度キーを押すと、オーバーレイを重ねるのではなく既存のピッカーにフォーカスします。

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
| `HERDR_PLUGIN_STATE_DIR` | どのピッカー pane が開いているかを記録し、2 度目の押下でフォーカスするため |
| `HERDR_PANE_ID` | pane プロセスにおける自身の pane ID。ピッカーが自分自身を一覧に出さないために使います |

アクションは、pane プロセスが自力では知り得ない次の 2 つを、開く pane に渡します。

| 変数 | 意味 |
| --- | --- |
| `GH_NAV_FROM_PANE` | ピッカーを呼び出した pane |
| `GH_NAV_REPO_ROOT` | 呼び出し元のリポジトリ（herdr が既に把握していた場合） |
