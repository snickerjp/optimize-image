use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process;

use image::codecs::jpeg::JpegDecoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── 設定ファイルの構造体 ──────────────────────────────────────────

#[derive(Deserialize)]
struct Config {
    sizes: Vec<SizeProfile>,
}

#[derive(Deserialize, Clone)]
struct SizeProfile {
    /// サイズ識別名（例: "thumbnail", "detail"）
    name: String,
    /// 最大幅（ピクセル）。画像が小さい場合は拡大しない
    max_width: u32,
    /// ファイルサイズ上限（KB）
    max_file_size_kb: u64,
    /// 品質初期値（1–100）。超過時は 5% ずつ 60% まで下げる
    initial_quality: u8,
    /// 出力先ディレクトリ（プロジェクトルートからの相対パス）
    output_dir: String,
    /// Web パス生成時に output_dir から除去するプレフィックス（省略可）
    #[serde(default)]
    strip_prefix: Option<String>,
}

impl Config {
    /// `path` から TOML 設定を読み込む
    fn load(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Cannot read config \"{}\": {e}", path.display()))?;
        toml::from_str(&content)
            .map_err(|e| format!("Invalid config \"{}\": {e}", path.display()))
    }
}

// ── JSON 出力の構造体 ──────────────────────────────────────────────

/// 生成された 1 サイズ分の結果
#[derive(Serialize, Clone)]
struct SizeResult {
    path: String,
    #[serde(rename = "sizeKb")]
    size_kb: u64,
}

/// stdout に出力する JSON（後方互換フィールド付き）
#[derive(Serialize)]
struct Output {
    filename: String,
    width: u32,
    height: u32,
    /// 生成されたサイズの結果マップ（size name → 結果）
    sizes: HashMap<String, SizeResult>,
    // ── backward-compat: thumbnail と detail が両方存在する場合のみ出力 ──
    #[serde(rename = "thumbnailPath", skip_serializing_if = "Option::is_none")]
    thumbnail_path: Option<String>,
    #[serde(rename = "detailPath", skip_serializing_if = "Option::is_none")]
    detail_path: Option<String>,
    #[serde(rename = "thumbnailSize", skip_serializing_if = "Option::is_none")]
    thumbnail_size: Option<u64>,
    #[serde(rename = "detailSize", skip_serializing_if = "Option::is_none")]
    detail_size: Option<u64>,
}

// ── カテゴリ正規化 ─────────────────────────────────────────────────

/// カテゴリ名を正規化（日本語 → 英語スラッグ）
fn normalize_category(category: &str) -> Result<&'static str, String> {
    match category.trim() {
        "オリジナル作品" => Ok("original"),
        "キャラクターデザイン" => Ok("character"),
        "ファンアート" => Ok("fanart"),
        "企業案件" => Ok("corporate"),
        "人物イラスト" => Ok("portrait"),
        "猫イラスト" => Ok("cat"),
        other => {
            let valid = "オリジナル作品, キャラクターデザイン, ファンアート, 企業案件, 人物イラスト, 猫イラスト";
            Err(format!(
                "Unknown category: \"{other}\"\nValid categories: {valid}"
            ))
        }
    }
}

// ── 画像処理ヘルパー ───────────────────────────────────────────────

/// JPEG ファイルから EXIF Orientation を読み取る
fn read_jpeg_orientation(path: &Path) -> Orientation {
    let Ok(file) = fs::File::open(path) else {
        return Orientation::NoTransforms;
    };
    let Ok(mut decoder) = JpegDecoder::new(BufReader::new(file)) else {
        return Orientation::NoTransforms;
    };
    decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms)
}

/// 画像を読み込み、EXIF Orientation に従って正規化する
fn load_image(path: &Path) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let mut img = ImageReader::open(path)?.with_guessed_format()?.decode()?;

    // JPEG の場合のみ EXIF Orientation を適用
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let orientation = if ext == "jpg" || ext == "jpeg" {
        read_jpeg_orientation(path)
    } else {
        Orientation::NoTransforms
    };

    img.apply_orientation(orientation);
    Ok(img)
}

/// 画像をリサイズ（拡大なし）
fn resize_image(img: &DynamicImage, target_width: u32) -> DynamicImage {
    if img.width() > target_width {
        img.resize(target_width, u32::MAX, FilterType::Lanczos3)
    } else {
        img.clone()
    }
}

/// 画像を WebP にエンコード（サイズ上限に収まるよう品質を自動調整）
fn encode_webp(
    img: &DynamicImage,
    max_size: usize,
    initial_quality: u8,
) -> Result<Vec<u8>, String> {
    let mut quality = initial_quality as i32;
    let mut last_result: Option<Vec<u8>> = None;

    // RGBA8 への変換はループ外で一度だけ行う
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let raw = rgba.as_raw();

    while quality >= 60 {
        let encoder = webp::Encoder::from_rgba(raw, width, height);
        let encoded = encoder.encode(quality as f32);
        let buffer = encoded.to_vec();

        let fits = buffer.len() <= max_size;
        last_result = Some(buffer);

        if fits {
            break;
        }
        quality -= 5;
    }

    match last_result {
        Some(buf) if buf.len() <= max_size => Ok(buf),
        Some(buf) => Err(format!(
            "Failed to optimize image to <= {max_size} bytes (last size: {} bytes at quality {}%)",
            buf.len(),
            quality + 5
        )),
        None => Err("No result produced".to_string()),
    }
}

// ── CLI 引数パーサー ───────────────────────────────────────────────

struct Args {
    image_path: String,
    category: String,
    config_path: PathBuf,
    sizes_filter: Option<Vec<String>>,
}

fn parse_args() -> Result<Args, String> {
    let raw: Vec<String> = std::env::args().collect();

    // 最低限: <image-path> と <category>
    if raw.len() < 3 || raw[1].starts_with("--") || raw[2].starts_with("--") {
        return Err(format!(
            "Usage: {} <image-path> <category> [--config <path>] [--sizes <name1,name2,...>]\n\
             Example: {} image.jpg オリジナル作品 --sizes thumbnail",
            raw[0], raw[0]
        ));
    }

    let image_path = raw[1].clone();
    let category = raw[2].clone();
    let mut config_path = PathBuf::from("optimize-image.toml");
    let mut sizes_filter: Option<Vec<String>> = None;

    let mut i = 3;
    while i < raw.len() {
        match raw[i].as_str() {
            "--config" => {
                i += 1;
                if i >= raw.len() {
                    return Err("--config requires a path argument".to_string());
                }
                config_path = PathBuf::from(&raw[i]);
            }
            "--sizes" => {
                i += 1;
                if i >= raw.len() {
                    return Err("--sizes requires a comma-separated list of size names".to_string());
                }
                sizes_filter = Some(
                    raw[i]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            unknown => {
                return Err(format!("Unknown option: {unknown}"));
            }
        }
        i += 1;
    }

    Ok(Args {
        image_path,
        category,
        config_path,
        sizes_filter,
    })
}

// ── エントリポイント ───────────────────────────────────────────────

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("❌ {e}");
        process::exit(1);
    });

    let image_path = Path::new(&args.image_path);

    let category_prefix = match normalize_category(&args.category) {
        Ok(prefix) => prefix,
        Err(e) => {
            eprintln!("❌ {e}");
            process::exit(1);
        }
    };

    // 設定ファイルを読み込む
    let config = Config::load(&args.config_path).unwrap_or_else(|e| {
        eprintln!("❌ {e}");
        process::exit(1);
    });

    // --sizes で絞り込み
    let profiles: Vec<SizeProfile> = if let Some(filter) = &args.sizes_filter {
        // 指定された名前が設定ファイルに存在するか検証
        for name in filter {
            if !config.sizes.iter().any(|s| &s.name == name) {
                let available: Vec<&str> = config.sizes.iter().map(|s| s.name.as_str()).collect();
                eprintln!(
                    "❌ Unknown size name: \"{name}\"\nAvailable sizes: {}",
                    available.join(", ")
                );
                process::exit(1);
            }
        }
        config
            .sizes
            .into_iter()
            .filter(|s| filter.contains(&s.name))
            .collect()
    } else {
        config.sizes
    };

    if profiles.is_empty() {
        eprintln!("❌ No size profiles to process.");
        process::exit(1);
    }

    eprintln!(
        "📷 Loading image: {}",
        image_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );

    let img = load_image(image_path)?;

    // ファイル名生成（カテゴリ-UUID 短縮 8 文字）
    let uuid = Uuid::new_v4().to_string().replace('-', "");
    let uuid_short = &uuid[..8];
    let filename = format!("{category_prefix}-{uuid_short}.webp");

    eprintln!("🎨 Optimizing images...");

    let root_dir = std::env::current_dir()?;
    let mut size_results: HashMap<String, SizeResult> = HashMap::new();
    // 出力する width/height は最大幅プロファイルのものを使用する
    let largest_profile = profiles
        .iter()
        .max_by_key(|p| p.max_width)
        .expect("profiles is non-empty");
    let largest_resized = resize_image(&img, largest_profile.max_width);
    let (out_width, out_height) = (largest_resized.width(), largest_resized.height());

    for profile in &profiles {
        let resized = if profile.name == largest_profile.name {
            largest_resized.clone()
        } else {
            resize_image(&img, profile.max_width)
        };

        let max_bytes = (profile.max_file_size_kb * 1024) as usize;
        let data = encode_webp(&resized, max_bytes, profile.initial_quality)
            .map_err(|e| format!("{}: {e}", profile.name))?;

        let out_dir = root_dir.join(&profile.output_dir);
        fs::create_dir_all(&out_dir)?;
        fs::write(out_dir.join(&filename), &data)?;

        eprintln!(
            "✅ {}: {} ({:.1}KB)",
            profile.name,
            filename,
            data.len() as f64 / 1024.0
        );

        let web_dir = if let Some(prefix) = &profile.strip_prefix {
            profile.output_dir.strip_prefix(prefix.as_str()).unwrap_or(&profile.output_dir)
        } else {
            &profile.output_dir
        };
        let web_path = format!("/{web_dir}/{filename}");
        size_results.insert(
            profile.name.clone(),
            SizeResult {
                path: web_path,
                size_kb: (data.len() as f64 / 1024.0).round() as u64,
            },
        );
    }

    eprintln!("\n✨ Optimization complete!");

    // 後方互換フィールド（thumbnail と detail が両方存在する場合のみ出力）
    let thumbnail_path = size_results.get("thumbnail").map(|r| r.path.clone());
    let detail_path = size_results.get("detail").map(|r| r.path.clone());
    let thumbnail_size = size_results.get("thumbnail").map(|r| r.size_kb);
    let detail_size = size_results.get("detail").map(|r| r.size_kb);

    let output = Output {
        filename,
        width: out_width,
        height: out_height,
        sizes: size_results,
        thumbnail_path,
        detail_path,
        thumbnail_size,
        detail_size,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("❌ Error: {e}");
        process::exit(1);
    }
}
