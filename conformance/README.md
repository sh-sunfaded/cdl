# 適合テストスイート

CDL の実装に依存しない形式で、言語仕様（`docs/SPEC.md`）への適合を確かめるテストを置く。

## ホスト

すべてのテストは、`HOST.md` に書いた参照ホスト（`host:contrast`）のもとで動かす。

## テストの形

- 1 ファイルのテスト: `<節>/<名前>.cdl` と、期待結果 `<節>/<名前>.expected.json` を並べて置く。
  `<節>` は SPEC.md の章番号（例: `5/switch-exhaustive.cdl`）。
- 禁止構文のテスト: `forbidden/<id>.cdl` と `forbidden/<id>.expected.json`。`<id>` は CLAUDE.md の 18 個
  （`let var class new this null for while try async enum as hooks event-handler ref destructuring rest-param and-nonboolean`）。
- 複数ファイルのテスト: `<節>/<名前>/` ディレクトリに、エントリの `main.cdl`、ほかのモジュール、期待結果 `expected.json` を置く。
- 名前に使えるのは ASCII の英小文字・数字・`-`。
- ファイルの先頭のコメントに、確かめる仕様の節と主張を書く。

## 期待結果

キーを 1 つだけ持つ JSON オブジェクト。

- `{"value": …}`: 評価に成功し、この値木になる。値木の形は DECISIONS D7 に従い、span は比べない。
- `{"diagnostics": […]}`: この診断の列になる（位置順）。各要素は `code`・`severity`・`span`（`file`・`start`・`end`）を持つ。
  比べるのはこの 3 つだけで、メッセージは書かない（実装ごとに違ってよい）。`file` は、1 ファイルのテストなら `<節>/<名前>.cdl`、
  複数ファイルのテストならディレクトリからの相対パス。

ランナーは、成功するテストを 2 回評価して結果が完全に一致することも確かめる（SPEC 第 12 章）。

## フックの呼び出し（SPEC 第 8 章）

`<節>/<名前>.calls.json`（複数ファイルのテストなら `calls.json`）に、フックの呼び出しの列を書く。
ランナーは評価に成功したあと、順に呼び出して結果を比べる。

```json
[
  { "hook": "windowOpen", "arg": { "appId": "firefox", "workspace": 1 }, "result": { "workspace": 2, "floating": false } },
  { "hook": "windowOpen", "arg": { "appId": "x" }, "error": "E4007" }
]
```

- `result` は戻り値（値木と同じ JSON の形）。フックが定義されていなければ `null`。
- `error` は診断のコード。`result` と `error` は、どちらか一方だけを書く。

## import の解決

複数ファイルのテストでは、`id` はテストのディレクトリからの相対パス（`parts/bind.cdl` など）。
`..` でテストのディレクトリの外へは出られない。

## テストの足し方

`.cdl` と期待結果を直接書いて足す（生成スクリプトはない。v0.1 で削除した）。診断の span は、実装の出力を目で確かめてから期待結果に書く。値木の期待結果は手で書く。ルートが関数の形のケースは、`<名前>.env.json`（複数ファイルのテストは `env.json`）で `Env` の値を渡す。
