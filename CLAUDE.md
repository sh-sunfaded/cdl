# CLAUDE.md — CDL (Contrast Document Language)

CDL は、Contrast（Rust + Smithay 製 Wayland コンポジタ）の設定とポリシーを宣言するための、TypeScript 風構文・JSX 風ドキュメントの静的型付き言語。
このリポジトリは Contrast とは独立しており、Contrast には外部ライブラリとして組み込まれる。

## 最初に読むもの

- `docs/SPEC.md` — 言語仕様（正典）。実装の判断はここに従う。
- `docs/DECISIONS.md` — 仕様が沈黙している点について、実装中に下した決定の記録（なければ作る）。

仕様と実装が食い違ったら、黙って実装を合わせない。食い違いを報告し、`docs/SPEC.md` と `docs/DECISIONS.md` のどちらをどう直すかを提案する。

## 現在の段階: プロトタイプ

- 目的は、仕様の主張（決定性・停止性・型安全）が実装で成立するかを早く確かめること。本番品質より、仕様の検証を優先する。
- プロトタイプに限り、構文解析に `oxc_parser` を使ってよい。ただし `cdl-syntax` の内側だけに閉じ込め、`oxc_*` の型を他のクレートに漏らさない（CDL 専用の AST に変換して渡す）。本実装では自前パーサに置き換える。
- プロトタイプの対象外: `fmt`、`migrate`、LSP、パッケージ管理。

## リポジトリ構成と依存の向き

```
crates/
  cdl-diag/    診断型（コード・重大度・span・メッセージ）とレンダリング
  cdl-syntax/  ソース → CDL AST（プロトタイプでは oxc_parser を内部で使用）
  cdl-sema/    名前解決・型検査・スキーマ適合
  cdl-eval/    評価器・リソース上限
  cdl-host/    組み込み API（Engine / ModuleResolver / Limits / Schema）
  cdl-cli/     check / eval
conformance/   .cdl と期待結果（値木 or 診断の JSON）。実装非依存
docs/          SPEC.md, DECISIONS.md
```

依存は一方向: `diag ← syntax ← sema ← eval ← host ← cli`。逆向き・飛び越しの依存を足さない。
公開 API の互換性を約束するのは `cdl-host` だけ。他のクレートは内部実装として自由に変えてよい。

## 言語の要点（詳細は SPEC.md）

- 構文は TSX のサブセット。設定は JSX 風の要素ツリーで書く: `export default (<Contrast>…</Contrast>);`
- 束縛は `const` のみ。ループ・クラス・`let`・`null`・`as`・Hooks・イベントハンドラ・`ref` は存在せず、使われたら診断する。
- 関数とコンポーネントは純関数で、再帰・相互再帰は名前解決でエラー。
- 評価は純粋・決定的で、必ず停止する。I/O・時刻・乱数・環境変数には触れない。
- 単位つきの値は組み込み関数で作る: `px(8)` `ms(120)` `color("#89b4fa")`。
- ホストのスキーマ（組み込みコンポーネントの props と子の型）は、Rust 側のビルダー API で登録する（プロトタイプの前提。`docs/DECISIONS.md` に記録する）。

## Rust のルール

- すべてのクレートに `#![forbid(unsafe_code)]`。
- clippy で deny: `unwrap_used` `expect_used` `panic` `indexing_slicing` `as_conversions`（縮小キャスト）`arithmetic_side_effects`。
- 公開 API は `Result` を返し、誤りは診断（`cdl-diag`）として値で返す。パニックと `abort` は禁止。
- `cdl-syntax` / `cdl-sema` / `cdl-eval` は、`std::fs` `std::env` `std::time` に依存しない。ファイルは `ModuleResolver` トレイト越しにだけ読む。
- 整数・数値演算は checked。`NaN` と `±Infinity` を生む演算は評価エラー。
- `Record` 相当のマップは `BTreeMap`（辞書順）。`HashMap` を値木に使わない（決定性のため）。
- 依存クレートは最小限。許可済み: `oxc_parser` と関連（`cdl-syntax` のみ）、`serde` / `serde_json`（`cdl-diag` と `cdl-cli`）、dev-dependencies の `proptest`。それ以外を足す前に確認する。

## 検証

仕様の主張ごとに検証手段を置く（SPEC.md 第 12 章）。

- Kani: 字句・構文の非パニック、ステップ上限の単調減少、checked 演算（`#[cfg(kani)]` のハーネス）。
- proptest: `fmt` 以外の往復性、型検査を通った式は評価で型エラーにならないこと。
- ゴールデン: `conformance/` の `.cdl` と期待 JSON。仕様の節番号をファイル名かコメントに入れる。
- 差分: 同じ入力を 2 回評価して結果が完全一致すること。

変更を出す前に、`scripts/check-all.sh` を実行する（中身は次のとおり）:

```
scripts/check-rules.sh          # 規則の静的検査（unsafe・unwrap・as・oxc の漏れ・依存の向き・fs/env/time・HashMap）
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/check-conformance.sh    # 決定性・禁止構文・診断コードの各帯
scripts/check-spec-examples.sh  # SPEC.md の CDL の例が通ること
WITH_KANI=1 scripts/check-all.sh  # Kani も含める（時間がかかる）
```

結果は OK / NG / 未実行 の 3 種類。入っていないツールは「未実行」であり、成功ではない。
未実行が残るあいだは、「完成」と報告しない。スクリプトは読み取り専用で、変更しない。

### conformance の置き方（スクリプトの前提）

- `conformance/<節>/<名前>.cdl` と、同じ名前の `<名前>.expected.json`（値木 or 診断）を並べる。
- 禁止構文は `conformance/forbidden/<id>.cdl`。`<id>` は `let var class new this null for while try async enum as hooks event-handler ref destructuring rest-param and-nonboolean`。
- cdl-cli は `cdl check|eval --json <file>` を持つ。`cdl check` は `--stage <syntax|sema|eval>` と、`--preset contrast`（examples のスキーマ）も受け付ける。

## 進め方

- 仕様を変える実装は、先に `conformance/` のテストを足してから実装する。
- 1 つの作業は小さく区切る。区切りごとに、動く状態でコミットする。
- 仕様にない機能を足さない。先回りした抽象化をしない。迷ったら入れない。
- 仕様が沈黙している点は、推測で進めず、選択肢を挙げて質問する。決まったら `docs/DECISIONS.md` に 1 項目で記録する。
- 診断コードは安定させる。意味を変えず、必要なら新しいコードを振る（帯は SPEC.md 第 10 章）。

## 破壊的な操作

`rm`、`git reset --hard`、`git push --force`、ファイルの上書き・削除は、1 件ずつ内容を示して承認を得てから実行する。
スクリプトやコマンド列を提案するときは、破壊的な操作を非破壊の操作と同じブロックに混ぜず、別のブロックに分ける。

## 言語

説明・コミットメッセージ本文・ドキュメントは日本語。識別子・コミットの種別（`feat:` `fix:` など）は英語。