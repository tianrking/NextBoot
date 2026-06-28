import lzma
import os

def make_smoke_linux_plugin_files(images):
    linux_image = next(
        (
            os.path.basename(image)
            for image in images
            if "linux" in os.path.basename(image).lower()
        ),
        "nextboot-smoke-linux.iso",
    )
    image_path = f"/ISO/{linux_image}"
    ventoy_json = f"""{{
  "auto_install": [
    {{
      "image": "{image_path}",
      "template": "/ventoy/autoinstall/linux.ks",
      "autosel": 1
    }}
  ],
  "persistence": [
    {{
      "image": "{image_path}",
      "backend": "/persistence/nextboot-linux.dat",
      "autosel": 1
    }}
  ],
  "injection": [
    {{
      "image": "{image_path}",
      "archive": "/ventoy/injection/tools.tar"
    }}
  ],
  "dud": [
    {{
      "image": "{image_path}",
      "dud": ["/ventoy/dud/dd.iso"]
    }}
  ]
}}
""".encode("utf-8")
    persistence_data = b"NEXTBOOT SMOKE PERSISTENCE\n"
    return [
        ("/ventoy/ventoy.json", ventoy_json),
        (
            "/ventoy/autoinstall/linux.ks",
            b"# NextBoot smoke auto-install template\nlang en_US.UTF-8\n",
        ),
        ("/ventoy/injection/tools.tar", b"NEXTBOOT SMOKE INJECTION ARCHIVE\n"),
        ("/ventoy/dud/dd.iso", b"NEXTBOOT SMOKE DUD IMAGE\n"),
        (
            "/persistence/nextboot-linux.dat",
            persistence_data + bytes(8192 - len(persistence_data)),
        ),
    ]

def make_smoke_conf_replace_files(images):
    smoke_image = next(
        (
            os.path.basename(image)
            for image in images
            if os.path.basename(image).lower().endswith(".iso")
        ),
        os.path.basename(images[0]) if images else "nextboot-smoke-efi.iso",
    )
    image_path = f"/ISO/{smoke_image}"
    ventoy_json = f"""{{
  "conf_replace": [
    {{
      "iso": "{image_path}",
      "org": "/CFG/GRUB.CFG",
      "new": "/ventoy/replace/grub.cfg"
    }},
    {{
      "iso": "{image_path}",
      "org": "/CFG/KICKSTART.CFG",
      "new": "/ventoy/replace/kickstart.cfg"
    }},
    {{
      "iso": "{image_path}",
      "org": "/CFG/AUTOINST.CFG",
      "new": "/ventoy/replace/autoinst.cfg"
    }}
  ]
}}
""".encode("utf-8")
    return [
        ("/ventoy/ventoy.json", ventoy_json),
        ("/ventoy/replace/grub.cfg", b"set timeout=1\nmenuentry 'patched' {}\n"),
        ("/ventoy/replace/kickstart.cfg", b"# patched smoke kickstart\n"),
        ("/ventoy/replace/autoinst.cfg", b"# patched smoke autoinstall\n"),
    ]

def make_smoke_auto_memdisk_files(images):
    smoke_image = next(
        (
            os.path.basename(image)
            for image in images
            if os.path.basename(image).lower() == "nextboot-smoke-efi.iso"
        ),
        os.path.basename(images[0]) if images else "nextboot-smoke-efi.iso",
    )
    ventoy_json = f"""{{
  "auto_memdisk": [
    "/ISO/{smoke_image}"
  ]
}}
""".encode("utf-8")
    return [("/ventoy/ventoy.json", ventoy_json)]

def make_smoke_windows_wimboot_files(helper):
    if not helper:
        raise SystemExit("windows wimboot smoke helper is missing")
    with open(helper, "rb") as src:
        helper_data = src.read()
    compressed = lzma.compress(
        helper_data,
        format=lzma.FORMAT_XZ,
        check=lzma.CHECK_CRC32,
        preset=0,
    )
    return [("/ventoy/wimboot.x86_64.xz", compressed)]
