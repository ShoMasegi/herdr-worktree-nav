# インストール

[English](../en/installation.md)

## 必要なもの

- **herdr 0.7.4 以降。** これより古いバージョンには `WorktreeInfo.open_workspace_id` と、プラグイン起動コンテキストの `worktree` フィールドがありません。このプラグインはどちらにも依存しています。
- **git。** `git rev-parse --path-format=absolute` が使えるバージョン（2.31 以降）。
- macOS または Linux。[Windows](#windows) を参照してください。

任意:

- **[`gh`](https://cli.github.com/)**（認証済み）。ブランチ一覧に pull request の番号とタイトルを表示します。これ以外の用途では使っておらず、失敗した場合はエラーにせず「pull request なし」として扱います。

## インストール

```sh
herdr plugin install ShoMasegi/herdr-worktree-nav
```

herdr は実行前にマニフェストと実行するコマンドを表示します。必ず目を通してください。プラグインは、あなたの環境で、あなたの権限で動く普通のコードです。

インストール後、アクションにキーを割り当ててください。[設定](configuration.md) を参照。

## ビルドステップが行うこと

`herdr plugin install` は確認後、`scripts/fetch-or-build.sh` を 1 回だけ実行します。

1. `herdr-plugin.toml` からバージョンを、`uname` からターゲットを判定します。
2. 対応する GitHub リリースから `herdr-worktree-nav-<version>-<target>.tar.gz` と `SHA256SUMS` をダウンロードし、アーカイブを検証します。
3. **いずれかが失敗したら**（リリースが無い・ネットワークが無い・未対応プラットフォーム・チェックサム不一致）、`cargo build --release` にフォールバックします。

そのため、ビルド済みバイナリがあれば Rust ツールチェーンは不要で、無い場合でもインストールは成立します。どちらの経路も使えない場合は、その旨を表示して中断します（中途半端な状態で終わりません）。

## 更新

```sh
herdr plugin uninstall herdr-worktree-nav
herdr plugin install ShoMasegi/herdr-worktree-nav
```

## アンインストール

```sh
herdr plugin uninstall herdr-worktree-nav
```

追加した `[[keys.command]]` を削除し、`herdr server reload-config` を実行してください。

herdr のプラグインディレクトリ以外には何も触れません。このプラグインが作成した worktree は普通の git worktree なので、そのまま残ります。消したい場合は `git worktree remove` か、herdr の「Delete worktree checkout」を使ってください。

## チェックアウトで開発する

```sh
git clone https://github.com/ShoMasegi/herdr-worktree-nav
cd herdr-worktree-nav
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-worktree-nav bin/herdr-worktree-nav
herdr plugin link .
```

`herdr plugin link` は `[[build]]` ステップを実行**しません**。そのため先に手でバイナリをビルドしています。以降は `cargo build --release` だけで十分です。シンボリックリンクが `bin/` を最新バイナリに向け続けるので、次にピッカーを開いたときには新しいコードが動きます。

リリース版に戻すには次を実行します。

```sh
herdr plugin unlink herdr-worktree-nav
```

## Windows

v1 では非対応です。理由は 2 つあり、どちらもフラグ 1 つでは済みません。

- Windows では herdr がプラグイン pane の相対コマンドを herdr 自身のディレクトリ基準で解決するため、エントリポイントに PowerShell 向けの絶対パス起動スクリプトが必要になります。
- herdr API へは Unix ドメインソケットで接続しており（[理由](../adr/0002-socket-transport.md)）、Windows では別のトランスポートが必要です。
