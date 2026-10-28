"""Build the Linux AppDir from an explicit allowlist; never copy user data."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
TOOLS = {
    "appimagetool-x86_64.AppImage": (
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage",
        "95cbe7cce9717fce90c484e34052ee7c7f1d7635b33c12525b4776826a7d29b6",
    ),
    "runtime-x86_64": (
        "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64",
        "617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46",
    ),
}
# glibc and its loader must come from the host as a consistent set.
HOST = re.compile(r"^(?:ld-linux.*|lib(?:c|m|mvec|dl|pthread|rt|resolv|util|anl)\.so\..*|libnss_.*)$")
DLOPEN = (
    "libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libX11.so.6",
    "libX11-xcb.so.1", "libXcursor.so.1", "libXrandr.so.2", "libXi.so.6",
    "libXinerama.so.1", "libxcb.so.1", "libxcb-render.so.0", "libxcb-shape.so.0",
    "libxcb-xfixes.so.0", "libwayland-client.so.0", "libwayland-cursor.so.0",
    "libwayland-egl.so.1",
)


def run(*command):
    return subprocess.check_output(command, text=True, stderr=subprocess.STDOUT)


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def copy(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target, follow_symlinks=True)


def tool(name):
    cache = Path.home() / ".cache/simneo-appimage-tools"
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / name
    url, digest = TOOLS[name]
    if not path.exists():
        with urllib.request.urlopen(url, timeout=120) as source, path.open("wb") as output:
            shutil.copyfileobj(source, output)
    if sha256(path) != digest:
        raise RuntimeError(f"SHA-256 inesperado para {name}; revise a ferramenta antes de atualizar o hash")
    path.chmod(0o755)
    return path


def package(binary):
    if os.uname().machine != "x86_64":
        raise RuntimeError("Este empacotamento é para Linux x86_64")
    if not binary.is_file():
        raise RuntimeError("Compile primeiro o binário Linux release")
    for command in ("file", "ldd", "objdump", "patchelf", "desktop-file-validate", "mksquashfs"):
        if not shutil.which(command):
            raise RuntimeError(f"Ferramenta de empacotamento ausente: {command}")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    output = ROOT / "dist" / f"SIMNeo-{version}-linux-x86_64.AppImage"
    output.parent.mkdir(exist_ok=True)
    image_tool, runtime = tool("appimagetool-x86_64.AppImage"), tool("runtime-x86_64")
    with tempfile.TemporaryDirectory(prefix="simneo-appimage-") as temporary:
        app = Path(temporary) / "SIMNeo.AppDir"
        libdir, bindir = app / "usr/lib", app / "usr/bin"
        libdir.mkdir(parents=True)
        bindir.mkdir()
        copy(binary, bindir / "sim-ui")
        copy(ROOT / "native/linux/t2u/build/libsim_t2u_linux.so", bindir / "assets/libsim_t2u_linux.so")
        for name in ("zenity", "secret-tool"):
            source = shutil.which(name)
            if not source:
                raise RuntimeError(f"Dependência de empacotamento ausente: {name}")
            copy(source, bindir / name)

        files = list(bindir.glob("*"))
        files = [path for path in files if path.is_file()]
        files.append(bindir / "assets/libsim_t2u_linux.so")
        known = set()
        originals = set()
        for name in DLOPEN:
            source = Path("/usr/lib/x86_64-linux-gnu") / name
            if not source.is_file():
                raise RuntimeError(f"Biblioteca de execução ausente: {name}")
            copy(source, libdir / name)
            originals.add(source.resolve())
            files.append(libdir / name)
            known.add(name)
        index = 0
        while index < len(files):
            path = files[index]
            index += 1
            dependencies = run("ldd", str(path))
            if "not found" in dependencies:
                raise RuntimeError(f"Dependência não resolvida em {path.name}: {dependencies}")
            for name, source in re.findall(r"^\s*(\S+) => (/\S+)", dependencies, re.MULTILINE):
                if HOST.match(name) or name in known:
                    continue
                known.add(name)
                originals.add(Path(source).resolve())
                destination = libdir / name
                copy(source, destination)
                files.append(destination)

        # All copied ELF objects use only relative paths inside the AppDir.
        for path in files:
            relative = os.path.relpath(libdir, path.parent)
            rpath = "$ORIGIN" if relative == "." else f"$ORIGIN/{relative}"
            subprocess.run(["patchelf", "--set-rpath", rpath, str(path)], check=True)
        linked_environment = dict(os.environ, LD_LIBRARY_PATH=str(libdir))
        for path in files:
            linked = subprocess.check_output(["ldd", str(path)], env=linked_environment, text=True)
            if "not found" in linked:
                raise RuntimeError(f"AppDir incompleto: {path.name}")
            for name, resolved in re.findall(r"^\s*(\S+) => (/\S+)", linked, re.MULTILINE):
                if not HOST.match(name) and not Path(resolved).is_relative_to(app):
                    raise RuntimeError(f"Dependência escapou do AppDir: {name}")
        copy(HERE / "AppRun", app / "AppRun")
        (app / "AppRun").chmod(0o755)
        copy(HERE / "simneo.desktop", app / "simneo.desktop")
        copy(HERE / "simneo.desktop", app / "usr/share/applications/simneo.desktop")
        copy(ROOT / "assets/icon.png", app / "simneo.png")
        copy(ROOT / "assets/icon.png", app / ".DirIcon")
        copy(HERE / "fonts.conf", app / "usr/share/simneo/fonts.conf")
        for font in ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf"):
            copy(Path("/usr/share/fonts/truetype/dejavu") / font, app / "usr/share/fonts" / font)
        for directory in ("glib-2.0/schemas", "icons/Adwaita"):
            shutil.copytree(Path("/usr/share") / directory, app / "usr/share" / directory, symlinks=False)
        copy(Path("/usr/share/icons/hicolor/index.theme"), app / "usr/share/icons/hicolor/index.theme")
        copy(ROOT / "assets/icon.png", app / "usr/share/icons/hicolor/512x512/apps/simneo.png")
        subprocess.run(["desktop-file-validate", str(app / "simneo.desktop")], check=True)

        packages = {}
        for source in sorted(originals | {Path("/usr/bin/zenity"), Path("/usr/bin/secret-tool")}):
            query = subprocess.run(["dpkg-query", "-S", str(source)], text=True, capture_output=True)
            if query.returncode:
                query = subprocess.run(["dpkg-query", "-S", str(source).replace("/usr/lib/", "/lib/")], text=True, capture_output=True)
            for line in query.stdout.splitlines():
                if ": /" not in line:
                    continue
                name = line.split(": /", 1)[0]
                packages[name] = run("dpkg-query", "-W", "-f=${Version}", name).strip()
        packages["fonts-dejavu-core"] = run("dpkg-query", "-W", "-f=${Version}", "fonts-dejavu-core").strip()
        for name in ("adwaita-icon-theme", "hicolor-icon-theme", "gsettings-desktop-schemas"):
            packages[name] = run("dpkg-query", "-W", "-f=${Version}", name).strip()
        for name in packages:
            copyright_file = Path("/usr/share/doc") / name.split(":")[0] / "copyright"
            if copyright_file.is_file():
                copy(copyright_file, app / "usr/share/doc/SIMNeo/licenses" / f"{name.replace(':', '_')}.txt")
        copy(ROOT / "LICENSE", app / "usr/share/doc/SIMNeo/LICENSE")
        copy(ROOT / "THIRD-PARTY.md", app / "usr/share/doc/SIMNeo/THIRD-PARTY.md")
        copy(ROOT / "README.md", app / "usr/share/doc/SIMNeo/README.md")
        cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
        registries = list((cargo_home / "registry/src").glob("*"))
        locked = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
        rust_licenses = app / "usr/share/doc/SIMNeo/licenses/rust"
        for package_info in locked:
            identity = f"{package_info['name']}-{package_info['version']}"
            source = next((registry / identity for registry in registries if (registry / identity).is_dir()), None)
            if source is None:
                continue
            for notice in source.iterdir():
                if re.match(r"^(LICENSE|LICENCE|LICENSING|COPYING|NOTICE|COPYRIGHT|UNLICENSE)", notice.name, re.IGNORECASE):
                    destination = rust_licenses / identity / notice.name
                    if notice.is_dir():
                        shutil.copytree(notice, destination)
                    else:
                        copy(notice, destination)
        if not (rust_licenses / "slint-1.18.1/LICENSES").is_dir():
            raise RuntimeError("Avisos de licença do Slint não encontrados no cache Cargo")
        glibc = set()
        for path in files:
            symbols = run("objdump", "-T", str(path))
            glibc.update(re.findall(r"GLIBC_(\d+\.\d+)", symbols))
        minimum = max(glibc, key=lambda value: tuple(map(int, value.split("."))))
        manifest = {
            "name": "SIMNeo", "version": version, "architecture": "x86_64",
            "minimum_glibc": minimum, "packages": dict(sorted(packages.items())),
            "tools_sha256": {name: digest for name, (_, digest) in TOOLS.items()},
        }
        (app / "usr/share/doc/SIMNeo/build-info.json").write_text(json.dumps(manifest, indent=2) + "\n")
        environment = dict(os.environ, ARCH="x86_64", VERSION=version)
        # Build in the temporary directory so a failed build cannot replace the artifact.
        temporary_image = Path(temporary) / output.name
        subprocess.run([
            str(image_tool), "--appimage-extract-and-run", "--no-appstream",
            "--runtime-file", str(runtime), "--mksquashfs-opt", "-processors",
            "--mksquashfs-opt", "1", str(app), str(temporary_image),
        ], env=environment, check=True)
        copy(temporary_image, output)
        output.chmod(0o755)
        (output.parent / f"{output.name}.sha256").write_text(f"{sha256(output)}  {output.name}\n")
        (output.parent / f"{output.stem}-build-info.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"AppImage: {output}\nTamanho: {output.stat().st_size} bytes\nglibc mínima: {minimum}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    package(parser.parse_args().binary.resolve())
