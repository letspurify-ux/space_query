use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Item, Lit, Meta, Token};

fn has_system_lib_via_pkg_config(name: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn build_empty_stub(out_dir: &Path, lib_name: &str) -> std::io::Result<()> {
    let src = out_dir.join(format!("{}_stub.c", lib_name));
    std::fs::write(&src, "void space_query_x11_stub(void) {}\n")?;

    let mut build = cc::Build::new();
    build.file(&src);
    build.warnings(false);
    build.compile(lib_name);
    Ok(())
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }

    Ok(())
}

struct CfgContext {
    target_os: String,
    debug_assertions: bool,
}

fn parse_cfg_items(list: &syn::MetaList) -> syn::Result<Punctuated<Meta, Token![,]>> {
    list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
}

fn meta_string_value(meta: &syn::MetaNameValue) -> Option<String> {
    match &meta.value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Some(value.value()),
        _ => None,
    }
}

fn eval_cfg(meta: &Meta, ctx: &CfgContext) -> syn::Result<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Ok(true),
        Meta::Path(path) if path.is_ident("debug_assertions") => Ok(ctx.debug_assertions),
        Meta::Path(path) if path.is_ident("unix") => Ok(ctx.target_os != "windows"),
        Meta::Path(path) if path.is_ident("windows") => Ok(ctx.target_os == "windows"),
        Meta::List(list) if list.path.is_ident("all") => {
            for item in parse_cfg_items(list)? {
                if !eval_cfg(&item, ctx)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Meta::List(list) if list.path.is_ident("any") => {
            for item in parse_cfg_items(list)? {
                if eval_cfg(&item, ctx)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Meta::List(list) if list.path.is_ident("not") => {
            let items = parse_cfg_items(list)?;
            let item = items.first().ok_or_else(|| {
                syn::Error::new_spanned(list, "cfg(not()) requires one predicate")
            })?;
            Ok(!eval_cfg(item, ctx)?)
        }
        Meta::NameValue(meta) if meta.path.is_ident("target_os") => {
            Ok(meta_string_value(meta).as_deref() == Some(ctx.target_os.as_str()))
        }
        Meta::NameValue(meta) if meta.path.is_ident("target_family") => {
            Ok(match meta_string_value(meta).as_deref() {
                Some("unix") => ctx.target_os != "windows",
                Some("windows") => ctx.target_os == "windows",
                _ => false,
            })
        }
        _ => Err(syn::Error::new_spanned(
            meta,
            "unsupported cfg predicate in test counter",
        )),
    }
}

fn item_is_enabled(attrs: &[syn::Attribute], ctx: &CfgContext) -> syn::Result<bool> {
    for attr in attrs {
        if !attr.path().is_ident("cfg") {
            continue;
        }

        let Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "expected #[cfg(...)] attribute",
            ));
        };

        for predicate in parse_cfg_items(list)? {
            if !eval_cfg(&predicate, ctx)? {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

fn count_test_items(items: &[Item], ctx: &CfgContext) -> syn::Result<usize> {
    let mut count = 0;

    for item in items {
        let attrs = match item {
            Item::Const(item) => &item.attrs,
            Item::Enum(item) => &item.attrs,
            Item::ExternCrate(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::ForeignMod(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Trait(item) => &item.attrs,
            Item::TraitAlias(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Use(item) => &item.attrs,
            _ => continue,
        };

        if !item_is_enabled(attrs, ctx)? {
            continue;
        }

        match item {
            Item::Fn(item_fn) => {
                if item_fn.attrs.iter().any(|attr| {
                    attr.path()
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "test")
                }) {
                    count += 1;
                }
            }
            Item::Mod(item_mod) => {
                if let Some((_, nested_items)) = &item_mod.content {
                    count += count_test_items(nested_items, ctx)?;
                }
            }
            _ => {}
        }
    }

    Ok(count)
}

fn count_rust_tests_in_dir(
    dir: &Path,
    ctx: &CfgContext,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_rust_files(dir, &mut files)?;

    let mut count = 0;
    for path in files {
        let source = fs::read_to_string(&path)?;
        let parsed = syn::parse_file(&source)?;
        count += count_test_items(&parsed.items, ctx)?;
    }

    Ok(count)
}

fn replace_patch_version(base_version: &str, patch: usize) -> String {
    let mut parts = base_version.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(major), Some(minor), Some(_), None) => format!("{major}.{minor}.{patch}"),
        _ => base_version.to_string(),
    }
}

fn configure_display_version() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tests");

    let cfg = CfgContext {
        target_os: env::var("CARGO_CFG_TARGET_OS")?,
        debug_assertions: env::var("DEBUG")
            .map(|value| value == "true")
            .unwrap_or(cfg!(debug_assertions)),
    };
    let base_version = env::var("CARGO_PKG_VERSION")?;
    let test_count = count_rust_tests_in_dir(Path::new("src"), &cfg)?
        + count_rust_tests_in_dir(Path::new("tests"), &cfg)?;
    let display_version = replace_patch_version(&base_version, test_count);

    println!("cargo:rustc-env=SPACE_QUERY_DISPLAY_VERSION={display_version}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    configure_display_version()?;

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "linux" {
        let out_dir = env::var("OUT_DIR")?;
        let out_path = Path::new(&out_dir).to_path_buf();

        let missing_xinerama = !has_system_lib_via_pkg_config("xinerama");
        let missing_xcursor = !has_system_lib_via_pkg_config("xcursor");
        let missing_xfixes = !has_system_lib_via_pkg_config("xfixes");
        let missing_xft = !has_system_lib_via_pkg_config("xft");

        if missing_xinerama {
            build_empty_stub(&out_path, "Xinerama")?;
        }
        if missing_xcursor {
            build_empty_stub(&out_path, "Xcursor")?;
        }
        if missing_xfixes {
            build_empty_stub(&out_path, "Xfixes")?;
        }
        if missing_xft {
            build_empty_stub(&out_path, "Xft")?;
        }

        if missing_xinerama || missing_xcursor || missing_xfixes || missing_xft {
            println!("cargo:warning=Missing X11 dev libs detected; linking local stubs for test/build in this environment.");
            println!("cargo:rustc-link-search=native={}", out_path.display());
        }
    }

    if target_os == "windows" {
        if let Err(e) = embed_windows_icon() {
            println!("cargo:warning=Failed to embed Windows icon resource: {}", e);
        }
    }

    Ok(())
}

// ── Windows icon embedding ────────────────────────────────────────────────

fn embed_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = env::var("OUT_DIR")?;
    let ico_path = Path::new(&out_dir).join("space_query.ico");
    write_ico_file(&ico_path)?;

    let mut res = winres::WindowsResource::new();
    res.set_icon(ico_path.to_str().ok_or("icon path is not valid UTF-8")?);
    res.compile()?;
    Ok(())
}

/// Write a multi-size ICO file to `path` using programmatically generated pixels.
fn write_ico_file(path: &Path) -> std::io::Result<()> {
    use std::io::Write;

    const SIZES: &[usize] = &[16, 32, 48, 256];

    // Generate RGBA pixel data for each size.
    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&sz| {
            let mut buf = vec![0u8; sz * sz * 4];
            fill_icon(&mut buf, sz);
            buf
        })
        .collect();

    // Encode each image as a BMP DIB (used inside ICO entries).
    let dibs: Vec<Vec<u8>> = images
        .iter()
        .zip(SIZES.iter())
        .map(|(rgba, &sz)| encode_bmp_dib(rgba, sz))
        .collect();

    // Compute image offsets within the ICO file.
    let header_size = 6 + SIZES.len() * 16;
    let mut offset = header_size as u32;
    let mut offsets = Vec::with_capacity(SIZES.len());
    for dib in &dibs {
        offsets.push(offset);
        offset += dib.len() as u32;
    }

    let mut f = std::fs::File::create(path)?;

    // ICO header: reserved=0, type=1 (icon), count=N
    f.write_all(&0u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&(SIZES.len() as u16).to_le_bytes())?;

    // ICONDIRENTRY × N
    for (i, &sz) in SIZES.iter().enumerate() {
        let w = if sz == 256 { 0u8 } else { sz as u8 };
        let h = if sz == 256 { 0u8 } else { sz as u8 };
        f.write_all(&[w, h, 0u8, 0u8])?; // width, height, colorCount, reserved
        f.write_all(&1u16.to_le_bytes())?; // planes
        f.write_all(&32u16.to_le_bytes())?; // bitCount
        f.write_all(&(dibs[i].len() as u32).to_le_bytes())?; // bytesInRes
        f.write_all(&offsets[i].to_le_bytes())?; // imageOffset
    }

    // Image data
    for dib in &dibs {
        f.write_all(dib)?;
    }

    Ok(())
}

/// Encode RGBA pixel data as a 32-bit BMP DIB suitable for embedding in an ICO.
/// Rows are written bottom-to-top (BMP convention) and channels are reordered to BGRA.
fn encode_bmp_dib(rgba: &[u8], size: usize) -> Vec<u8> {
    // AND mask: 1-bit per pixel, padded to DWORD rows.  All zeros = fully opaque
    // at the legacy mask level; real transparency is carried by the alpha channel.
    let and_row_stride = size.div_ceil(32) * 4;
    let and_size = and_row_stride * size;
    let total = 40 + size * size * 4 + and_size;
    let mut out = Vec::with_capacity(total);

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (*2 = BMP-in-ICO)
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression (BI_RGB, BGRA in practice)
    out.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // Pixel data: BGRA, bottom-to-top row order
    for row in (0..size).rev() {
        for col in 0..size {
            let idx = (row * size + col) * 4;
            out.push(rgba[idx + 2]); // B
            out.push(rgba[idx + 1]); // G
            out.push(rgba[idx]); // R
            out.push(rgba[idx + 3]); // A
        }
    }

    // AND mask (all zeros)
    out.extend(std::iter::repeat_n(0u8, and_size));

    out
}

// ── Icon pixel generation (shared with src/app_icon.rs via icon_fill.rs) ──

fn safe_div(lhs: f32, rhs: f32) -> f32 {
    if rhs.abs() <= f32::EPSILON {
        0.0
    } else {
        lhs / rhs
    }
}

include!("src/icon_fill.rs");
