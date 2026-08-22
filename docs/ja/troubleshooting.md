# トラブルシューティング

[English](../en/troubleshooting.md)

## まずここから

```sh
herdr plugin log list --plugin herdr-gh-nav --limit 5
```

herdr は実行したプラグインコマンドを、終了コードと stderr つきで記録しています。ピッカーが出てこない場合、ほぼ必ずここに理由が残っています。

```sh
herdr-gh-nav dump
```

Panes ビューが描画するはずのツリーを出力します。`dump` が正しくてピッカーが誤っていれば描画の問題、`dump` の時点で誤っていれば herdr か git が返した内容の問題です。`HERDR_SOCKET_PATH` が必要なので herdr の pane 内から実行してください。

## キーを押しても何も起きない

まずアクションが登録されているか確認します。

```sh
herdr plugin action list --plugin herdr-gh-nav
```

アクションは出ているのにキーが効かない場合は、プラグインではなくキーバインドの問題です。`[[keys.command]]` の記述を確認し、`herdr server reload-config` を実行してください。切り分けにはアクションを直接実行します。

```sh
herdr plugin action invoke herdr-gh-nav.open-panes
```

## 「Unable to spawn … because it does not exist」

`bin/` にバイナリがありません。開発用チェックアウトの場合:

```sh
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-gh-nav bin/herdr-gh-nav
```

`herdr plugin link` はビルドステップを実行しないため、link したチェックアウトでは手動でビルドする必要があります。

インストール済みのプラグインの場合は入れ直してください。ビルドステップが取得またはビルドします。

```sh
herdr plugin uninstall herdr-gh-nav && herdr plugin install ShoMasegi/herdr-gh-nav
```

## 「HERDR_SOCKET_PATH is not set」

herdr からではなくシェルから直接実行しています。このバイナリは単体ツールではなく、herdr がプラグインコマンドに渡すソケットを必要とします。herdr セッション内の pane から実行するか、アクション経由で起動してください。

## pane が違うリポジトリに出る／出てこない

Panes ビューは pane の作業ディレクトリ（フォアグラウンドプロセスのもの、無ければシェルのもの）でグループ化します。`cd` で移動した pane は移動先でグループ化されます。多くの場合これが期待どおりですが、意外に感じることもあります。

herdr が何を返しているか確認します。

```sh
herdr pane get <pane_id>
```

`cwd` と `foreground_cwd` の両方が無い場合、herdr はその pane の中を見られていないため、一覧の末尾にある「not in any repository」セクションに入ります。

## GitHub にあるはずのブランチが出てこない

リモート一覧の取得にはネットワークと git の認証情報が必要です。

```sh
git ls-remote --heads origin
```

これが失敗したりハングしたりする場合、ピッカーの背景取得も同様です。その場合は静かに諦め、ローカルの一覧をそのまま表示します。`origin` が無いリポジトリでローカルブランチのみが出るのは仕様です。

## pull request が表示されない

任意機能であり、失敗しても致命的にはなりません。ピッカーから見えている内容を確認してください。

```sh
gh auth status
gh pr list --json number,title,headRefName,isDraft
```

`gh` が無い、認証が通っていない、GitHub のリポジトリではない、のいずれかであればこの列は単に出ません。

## worktree が想定と違う場所に作られた

作成場所はこのプラグインではなく herdr の設定です。

```sh
herdr --default-config | grep -A2 '\[worktrees\]'
```

checkout は `<directory>/<repo>/<branch-slug>` に置かれます。変更するには herdr の設定で `[worktrees] directory` を設定してください（[設定](configuration.md) 参照）。

## 手動確認チェックリスト

herdr 側は CI ではテストできません（サーバーが無いため）。リリース前に実セッションで一通り確認してください。

- [ ] `herdr plugin link .` で 2 つのアクションと 2 つの pane エントリポイントが登録される（`herdr plugin list --json`）
- [ ] ピッカーが herdr の枠付き（タイトル `herdr-gh-nav`）の中央寄せ popup として開き、周囲にセッションが見えたままで、**自分自身は一覧に出ない**
- [ ] pane の無い worktree が `no pane` 付きで出て、`Enter` で開ける
- [ ] `↑`/`↓` が pane と「何も動いていない checkout」にだけ止まる。リポジトリの見出しと、既に pane を持つ checkout は飛ばされ、表示自体は残る
- [ ] `←`/`→` が 1 押しで 1 リポジトリ動き、その最初の pane または最初の idle checkout に着く。端で巻き戻り、リポジトリに属さない pane 群も対象に含まれる
- [ ] 別の space の pane で `Enter` を押すとそこへ移動し、popup が閉じた後もそこに留まる
- [ ] `n` でカーソル位置の checkout に pane が追加される
- [ ] `Tab` で Branches ビューに行け、カーソル位置のリポジトリから始まる
- [ ] Branches ビューに herdr が開いているリポジトリがすべて出て、呼び出し元に印が付き、そこにカーソルがある
- [ ] 別のリポジトリを選ぶとそのブランチが出る。`Esc` で一覧に戻り、一度読んだリポジトリに戻っても git が再実行されない
- [ ] `i` が state / updated / name を巡回し、`Shift-I` が反転する。どちらもカーソルが新しい先頭行に乗る。入力中は `Ctrl-O` / `Ctrl-R` が同じ働きをし、並び順はリポジトリ切り替えをまたいで保たれる
- [ ] Branches ビューが、プラグインディレクトリからだけでなく **worktree の中から** も開ける（相対パスの pane コマンドを検出できるケース）
- [ ] 未 fetch のリモートブランチが fetch され、worktree が `HEAD` ではなく `origin/<branch>` を基点に作られる（新しい checkout で `git log --oneline -1` を確認）
- [ ] Branches がコマンドモードで開く。`j`/`k` で移動し、`f`/`o`/`r`/`q` がキーヒントどおりに効き、`/` を押して初めて英字が検索フィールドに入る
- [ ] `Ctrl-F` で fetch できる。`fetching origin…` が出て、`ls-remote` しか知らなかったブランチに日付と件名が付き、リモートで削除済みのブランチが一覧から消える
- [ ] その処理中、ピッカーが画面に残り、今の段階名を表示し、スピナーが回る。`Ctrl-C` は fetch 中は効き、`working…` 表示中は効かない
- [ ] 失敗した段階で画面が保持され、git または herdr の言い分がそのまま出て、`Enter` か `Esc` で閉じる。同じメッセージが `herdr plugin log list` にも残る。リモートに到達できないことが「not a git repository」と報告されないこと
- [ ] すでに pane で開いているブランチは、二重にチェックアウトせずジャンプする
- [ ] 4 つの行き先すべてが動く: ここに split / 既存 tab / 既存 space / 新規 space
- [ ] 作成して移動した後、`herdr workspace list` に余分な workspace が残っておらず、checkout はディスク上に残っている
- [ ] herdr サーバーを停止した状態でバイナリを実行すると、panic せず説明を出して終了する
- [ ] `Tab` を押しても枠のタイトルは変わらず（ビュー名ではなくプラグイン名のため）、検索行とキーヒントだけがビューに追従する

popup はアドレス指定できないため、`herdr pane read` や `herdr pane send-keys` でピッカーを操作できません。同じコードをキー入力付きで確認するには、通常の pane で `./bin/herdr-gh-nav pane panes` を実行してください。枠だけは目視で確認する必要があります。
