# 使い方

[English](../en/usage.md)

ピッカーは 2 つ、それぞれ対応するアクションで開き、`Tab` で切り替えます。どちらもオーバーレイで、開いている間はセッションを覆い、閉じた瞬間に消えます。

描画は herdr 本体の session navigator と同じ方式です。パネル・検索行・詳細行・キーヒントの構成、tree グリフ、状態グリフ、そして herdr のテーマの accent 色まで揃えてあります。navigator が定めているキーは、ここでも同じ意味を持ちます。

**どこから開いたか**が重要です。キーを押した時点のリポジトリと pane を引き継ぐので、「ここに split」の「ここ」が決まり、ブランチ一覧の対象リポジトリも決まります。

## Panes

| キー | 動作 |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | 移動 |
| pane 行で `Enter` | そこへ移動 |
| pane がある worktree 行で `Enter` | その最初の pane へ移動 |
| pane が無い worktree 行で `Enter` | その checkout を開く |
| リポジトリ行で `Enter` | 折りたたむ／展開する |
| `n` | カーソル位置の checkout に pane を追加 |
| `Tab` | カーソル位置のリポジトリの Branches へ |
| `/` | 検索 |
| `b` `w` `i` `d` | blocked / working / idle / done に絞り込み |
| `a` | 状態の絞り込みを解除 |
| `h` | リポジトリ外の pane の表示を切り替え |
| `r` | 再読み込み |
| `q`, `Esc`, `Ctrl-C` | 閉じる |

`b`/`w`/`i`/`d` を押すと、検索ボックスが状態チップに変わります。同じキーをもう一度押すと解除されるので、絞り込みが一方通行になることはありません。

検索中は、英字はコマンドではなく文字として入力されます。`Enter` は絞り込みを残したまま上記のキー操作に戻り、`Esc` は絞り込みを破棄し、`Ctrl-U` は検索モードのまま中身だけ空にします。

### 行の読み方

```
 ◆ ▾ ● ShoMasegi/herdr-gh-nav (2)               1 working
   └── ● main                                   2 panes · 1 working
 ◆    ├── ● claude                              claude · working
      └── · shell                               shell
   └── · fix/crash                              no pane
```

左から順に、gutter、tree、状態グリフ、ラベル、meta 列です。

- **gutter** には、いまフォーカスされている pane と、それを含むリポジトリに `◆` が付きます。
- **tree** はリポジトリに `▾`/`▸`（`Enter` で開閉）、その配下に `├──`/`└──` の連結グリフを使います。階層が深い worktree でも、どこに属しているかが読み取れます。
- **ラベル** はリポジトリが `owner/repo (n)`（`n` は開いている pane 数）、worktree はブランチ名、pane はエージェント名（無ければ `shell`）です。
- **meta 列** は「ファイルがどこにあるか」ではなく「いま何が起きているか」を示します。リポジトリは活動サマリ、worktree は `n panes · 活動`、pane は `エージェント · 状態` です。何も動いていない checkout は `no pane` と出ます。

checkout パスは一覧の下の詳細行に出て、カーソルに追従します。行からパスを外しているのは、一覧を「活動」の観点で一目で追えるようにするためです。

リポジトリごとに空行が入り、一覧が pane より長い場合は右端にスクロールバーが出ます。

エージェントの状態: `●` 実行中、`○` 待機中、`◆` ブロック中、`·` エージェントなし。herdr が `status_indicators = "symbols"` の場合は `◐`、`○`、`×`、`·` になります。

### 絞り込み

絞り込みは fuzzy で、ツリーの下方向にカスケードします。リポジトリにマッチすればその中身がすべて出ます。worktree にマッチすればその上で動いている pane も出ます。pane 単独でマッチした場合は、それがどこにあるか分かるよう上位の見出しも一緒に出ます。

自分自身はマッチせず、文脈として残っている行（結果の上のリポジトリ、マッチしたブランチ配下の pane）は一覧から消さず、dim 表示で残します。結果が「どこにあるか」を説明する構造なしに出ることはありません。

fuzzy マッチは緩く、`harken` は意外なほど多くの文字列の部分列になります。そのため結果はツリー順ではなくマッチの良さ順に並べます。意図したリポジトリが先頭に来ます。（herdr の navigator はセッション順を保ちます。あちらの行順は利用者が既に知っている順序ですが、こちらはそうではないためです。）

## Branches

ブランチ一覧そのものが検索ボックスです。入力モードに入る操作はありません。ブランチ名を打つのが最も多い操作だからです。

| キー | 動作 |
| --- | --- |
| 英数字 | 絞り込み |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | 移動 |
| `Enter` | このブランチを選ぶ |
| `Backspace` | 1 文字削除 |
| `Ctrl-U` | 入力を空にする |
| `Tab` | Panes に戻る |
| `Esc`, `Ctrl-C` | 閉じる |

リモートは背景で読み込みます。ローカルの結果は即座に表示され、`git ls-remote` が返るまでプロンプト脇に `reading the remote…` が出ます。未 fetch のブランチはそれが返った時点で追加されます。オフラインの場合はこの行が消えるだけで、ローカルの一覧はそのまま使えます。

### 各状態の意味

| 表示 | ブランチの状態 | `Enter` の動作 |
| --- | --- | --- |
| `● running` | いま pane で開いている | その pane へ移動 |
| `○ checked out` | worktree はあるが何も動いていない | 指定した場所にその checkout を開く |
| `· local` | ローカルブランチ、worktree なし | そこから worktree を作る |
| `↓ remote` | リモートにあるが未 fetch | fetch してから `origin/<branch>` を基点に作る |
| `+ create` | まだ存在しない（入力した名前） | `HEAD` から作成し、そこから worktree を作る |

`running` は行き先の選択を飛ばします。その作業はすでに開いているので、「二重に開いた分をどこに置くか」を尋ねるのは筋が違うためです。

`remote` は `refs/remotes/origin/<branch>` に fetch し、その ref を基点に worktree を作ります。`HEAD` を基点にすると、GitHub 上のブランチと名前だけ同じ空のブランチができてしまいます。

作成候補は常に末尾に出ます。入力が有効なブランチ名で、かつ同名のブランチが存在しなければ、他に fuzzy マッチがあっても出ます。`feat/login` が存在するときに `feat/login-v2` を作れなくなってはいけないためです。

### 行き先を選ぶ

| キー | 動作 |
| --- | --- |
| `↑` `↓`, `k` `j` | 移動 |
| `Enter` | そこに pane を開く |
| `Esc`, `Backspace` | ブランチ一覧に戻る |

```
here            split right
                split down
existing tab    w1  app / logs
                w5  harken / android
existing space  w1  app → new tab
new space       on its own
```

- **here** はピッカーを呼び出した pane を split します。
- **existing tab** はその tab を、herdr が適当と判断した pane で split します。呼び出し元の tab は一覧に出ません（「here」がそれに当たるためです）。
- **existing space** はその space に tab を追加します。
- **new space** は herdr が作った workspace のまま残します。herdr 本体の `new worktree` と同じ挙動です。

`split right` が最初から選択されているので、`Enter` `Enter` で作業中の隣にブランチが並びます。

## 診断

```sh
herdr-gh-nav dump
```

ピッカーが描画するはずのツリーをプレーンテキストで出力します。「herdr や git が妙な値を返している」のか「描画が間違っている」のかを切り分けるのに使えます。`HERDR_SOCKET_PATH` が必要なので、herdr セッション内の pane から実行してください。
