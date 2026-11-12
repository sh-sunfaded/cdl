# 参照ホスト

適合テストは、次のホストのもとで動かす。実装は、このスキーマを自分のホスト API で登録してテストを走らせる。
ここに書く型は、CDL から見た型の説明である（DECISIONS E1）。

## `host:contrast`

ルート要素: `Contrast`

```ts
export declare function Contrast(props: {
  children?: Element<Appearance | Animation | Keybind | Label | Group>[];
}): Element<Contrast>;

// 一意な子: Contrast の子の中に 1 つまで（DECISIONS E2）
export declare function Appearance(props: {
  gap: Length; radius: Length; focusColor?: Color;
}): Element<Appearance>;

export declare function Animation(props: {
  duration: Duration; enabled?: boolean;
}): Element<Animation>;

export declare function Keybind(props: {
  keys: string; action: string;
}): Element<Keybind>;

// テキスト子（`<Label>text</Label>` と `{"text"}`）を受け付ける唯一のコンポーネント（DECISIONS R1）。
// ホスト API では `ComponentDef::text_children()` で宣言する。
export declare function Label(props: {
  children?: string[];
}): Element<Label>;

// スロット（要素を渡す props）を持ち、子に要素（Keybind）とテキストを混在できる（DECISIONS U2）。
export declare function Group(props: {
  header?: Element<Label>;
  footer?: Element<Label>[];
  children?: (Element<Keybind> | string)[];
}): Element<Group>;

// ホストの環境（ルートを関数の形で書くときの引数。`SchemaBuilder::env` で宣言する。DECISIONS U1）。
// 適合テストは、ルートが関数の形のケースに `<名前>.env.json`（`env.json`）で Env の値を渡す。
export interface Env {
  output: { name: string; width: number; height: number; scale: number };
}

export interface WindowInfo { appId: string; workspace: number }
export interface Placement { workspace: number; floating: boolean }
export interface Policy {
  windowOpen?: (w: WindowInfo) => Placement;
}
```

一意な子: `Appearance`。
