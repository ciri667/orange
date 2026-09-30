use std::fs;
use std::path::{Path, PathBuf};

/** Tauri 构建入口，负责生成桌面端所需的上下文代码。 */
fn main() {
    // sherpa 的构建脚本会把 DLL 拷到 target/<profile>。这里再拷一次，避免增量编译没重跑依赖脚本时调试目录缺库。
    #[cfg(windows)]
    stage_sherpa_runtime_dlls();
    tauri_build::build();
}

/** 把 sherpa-onnx 预编译 DLL 放到可执行文件旁边，Windows 加载器才能在启动时找到它们。 */
#[cfg(windows)]
fn stage_sherpa_runtime_dlls() {
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let out_dir = PathBuf::from(out_dir);
    let Some(build_dir) = out_dir
        .ancestors()
        .find(|path| path.file_name() == Some("build".as_ref()))
    else {
        return;
    };
    let Some(profile_dir) = build_dir.parent() else {
        return;
    };
    let Some(target_dir) = profile_dir.parent() else {
        return;
    };
    let Some(lib_dir) = find_sherpa_lib_dir(&target_dir.join("sherpa-onnx-prebuilt")) else {
        println!("cargo:warning=还没有 sherpa-onnx 预编译库，跳过 DLL 拷贝");
        return;
    };

    let mut copied = 0_u32;
    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("dll") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        if fs::copy(&path, profile_dir.join(name)).is_ok() {
            copied += 1;
        }
    }
    println!(
        "cargo:warning=已将 {copied} 个 sherpa-onnx DLL 复制到 {}",
        profile_dir.display()
    );
}

/** 在预编译解压目录里查找含 onnxruntime.dll 的 lib 目录。 */
#[cfg(windows)]
fn find_sherpa_lib_dir(root: &Path) -> Option<PathBuf> {
    let direct = root
        .join("sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib")
        .join("lib");
    if direct.join("onnxruntime.dll").is_file() {
        return Some(direct);
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join("lib");
        if candidate.join("onnxruntime.dll").is_file() {
            return Some(candidate);
        }
    }
    None
}
