# 使い方

[English](../en/usage.md)

ピッカーは 2 つ、それぞれ対応するアクションで開き、`Tab` で切り替えます。どちらもオーバーレイで、開いている間はセッションを覆い、閉じた瞬間に消えます。

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
| `/` | 絞り込み |
| `h` | リポジトリ外の pane の表示を切り替え |
| `r` | 再読み込み |
| `q`, `Esc`, `Ctrl-C` | 閉じる |

絞り込み中は、英字はコマンドではなく文字として入力されます。`Enter` は絞り込みを残したまま上記のキー操作に戻り、`Esc` は絞り込みを消します。

### 行の読み方

```
▾ ShoMasegi/herdr-gh-nav                          ~/Workspace/herdr-gh-nav
  ● main                                          ~/Workspace/herdr-gh-nav
    ● claude                                                         w7:p2
  ○ fix/crash  no pane           ~/.herdr/worktrees/herdr-gh-nav/fix-crash
```

- リポジトリ名は、`origin` が GitHub なら `owner/repo`、そうでなければディレクトリ名です。パスは折りたたまれているときだけ表示します。展開時は直下の主 checkout が同じパスを持っているためです。
- `●` は主 checkout、`○` はリンクされた worktree です。
- `no pane` はその checkout で何も動いていないことを表します。`Enter` で開きます。
- pane 行にはエージェント名（herdr がエージェントを認識していない場合は `shell`）と、右端に pane ID が出ます。

エージェントの状態: `●` 実行中、`○` 待機中、`◆` ブロック中、`✓` 完了、`·` エージェントなし。

### 絞り込み

絞り込みは fuzzy で、ツリーの下方向にカスケードします。リポジトリにマッチすればその中身がすべて出ます。worktree にマッチすればその上で動いている pane も出ます。pane 単独でマッチした場合は、それがどこにあるか分かるよう上位の見出しも一緒に出ます。

fuzzy マッチは緩く、`harken` は意外なほど多くの文字列の部分列になります。そのため結果はツリー順ではなくマッチの良さ順に並べます。意図したリポジトリが先頭に来ます。

## Branches

ブランチ一覧そのものが検索ボックスです。入力モードに入る操作はありません。ブランチ名を打つのが最も多い操作だからです。

| キー | 動作 |
| --- | --- |
| 英数字 | 絞り込み |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | 移動 |
| `Enter` | このブランチを選ぶ |
| `Backspace` | 1 文字削除 |
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
