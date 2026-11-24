use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/app.rc");
    println!("cargo:rerun-if-changed=packaging/windows/app.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = manifest_dir.join("packaging/windows/app.rc");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    let linked_resource = match target_env.as_str() {
        "msvc" => compile_msvc_resource(&rc_path, &out_dir),
        "gnu" => compile_gnu_resource(&rc_path, &out_dir),
        other => panic!("Unsupported Windows target env for icon resource embedding: {other}"),
    };

    println!(
        "cargo:rustc-link-arg-bin=twitch-streak-saver={}",
        linked_resource.display()
    );
}

fn compile_msvc_resource(rc_path: &Path, out_dir: &Path) -> PathBuf {
    let output = out_dir.join("app.res");
    let rc_exe = find_msvc_rc_exe().expect("Could not locate rc.exe in PATH or Windows Kits");
    let include_dir = rc_path.parent().expect("app.rc parent");
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/i")
        .arg(include_dir)
        .arg("/fo")
        .arg(&output)
        .arg(rc_path)
        .status()
        .unwrap_or_else(|error| panic!("Failed to run {}: {error}", rc_exe.display()));
    assert!(
        status.success(),
        "rc.exe failed to compile {}",
        rc_path.display()
    );
    output
}

fn find_msvc_rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let program_files_x86 = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let kits_bin = program_files_x86.join(r"Windows Kits\10\bin");
    let mut versions = std::fs::read_dir(&kits_bin)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join(arch).join("rc.exe"))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    versions.sort();
    versions.pop().or_else(|| {
        let direct = kits_bin.join(arch).join("rc.exe");
        direct.is_file().then_some(direct)
    })
}

fn compile_gnu_resource(rc_path: &Path, out_dir: &Path) -> PathBuf {
    let output = out_dir.join("app.o");
    let windres = ["x86_64-w64-mingw32-windres", "windres"]
        .into_iter()
        .find(|candidate| Command::new(candidate).arg("--version").output().is_ok())
        .expect("Could not locate x86_64-w64-mingw32-windres or windres");
    let include_dir = rc_path.parent().expect("app.rc parent");
    let status = Command::new(windres)
        .arg("-I")
        .arg(include_dir)
        .arg("-i")
        .arg(rc_path)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&output)
        .status()
        .unwrap_or_else(|error| panic!("Failed to run {windres}: {error}"));
    assert!(
        status.success(),
        "{windres} failed to compile {}",
        rc_path.display()
    );
    output
}
