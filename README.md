# optimize-image

画像を WebP 形式に変換・最適化する Rust 製 CLI ツール。

## 機能

- JPEG / PNG / GIF / BMP / TIFF → WebP 変換
- JPEG EXIF Orientation 自動回転
- 設定ファイルで複数サイズプロファイルを定義
- ファイルサイズ上限に収まるよう品質を自動調整
- JSON 形式で結果を stdout に出力

## 前提条件

- Rust toolchain
- libwebp-dev（Ubuntu: `apt install libwebp-dev` / macOS: `brew install webp`）

## ビルド

```bash
cargo build --release
```

## 使い方

```bash
# 全サイズプロファイルで生成
optimize-image image.jpg "オリジナル作品"

# サムネイルのみ生成
optimize-image image.jpg "オリジナル作品" --sizes thumbnail

# カスタム設定ファイルを指定
optimize-image image.jpg "オリジナル作品" --config /path/to/config.toml
```

## カテゴリ一覧

| 日本語 | スラッグ |
|--------|----------|
| オリジナル作品 | original |
| キャラクターデザイン | character |
| ファンアート | fanart |
| 企業案件 | corporate |
| 人物イラスト | portrait |
| 猫イラスト | cat |

## 設定ファイル（optimize-image.toml）

```toml
[[sizes]]
name = "thumbnail"
max_width = 500
max_file_size_kb = 50
initial_quality = 75
output_dir = "output/thumbnails"
strip_prefix = ""  # Web パス生成時に除去するプレフィックス（省略可）

[[sizes]]
name = "detail"
max_width = 1200
max_file_size_kb = 800
initial_quality = 80
output_dir = "output/detail"
```

## 出力例

```json
{
  "filename": "original-e727ea33.webp",
  "width": 1200,
  "height": 1200,
  "sizes": {
    "thumbnail": { "path": "/output/thumbnails/original-e727ea33.webp", "sizeKb": 23 },
    "detail":    { "path": "/output/detail/original-e727ea33.webp",    "sizeKb": 79 }
  },
  "thumbnailPath": "/output/thumbnails/original-e727ea33.webp",
  "detailPath": "/output/detail/original-e727ea33.webp",
  "thumbnailSize": 23,
  "detailSize": 79
}
```
