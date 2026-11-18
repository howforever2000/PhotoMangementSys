"""OpenVINO 运行时的**发现与解压**逻辑（BUG-2026-1108-007）。

这两个坑都不是"手滑"，而是产品行为错了：

1. **放错位置就静默**：zip 只在 `<模型目录>/_runtime` 被认，用户按 README 里
   `model.zip`「解压到安装根目录」的思路，把组件放到了 `<安装根目录>/_runtime`
   （差两级）→ 一句「NPU 不可用」，没有任何线索。现在顺着 MODEL_DIR 往上三层都认。
2. **半成品目录被当成可用**：删除/杀毒打断过的 `ovrt` 仍会让
   `import openvino` "成功"（拿到 namespace package），直到 `ov.Core()` 才报
   `AttributeError: module 'openvino' has no attribute 'Core'`。
   现在用 `openvino/__init__.py` 当哨兵，不完整就重新解压。
"""

import os
import zipfile

import pytest

from vcr import config, ov_backend as ovb


def _make_zip(path: str, with_init: bool = True) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("openvino/libs/dummy.txt", "x")
        if with_init:
            zf.writestr("openvino/__init__.py", "# fake\n")
            zf.writestr("openvino/_pyopenvino.cp312-win_amd64.pyd", "fake-pyd")


@pytest.fixture()
def install(tmp_path, monkeypatch):
    """造一个 <install>/vcr/models 的安装布局，并把 MODEL_DIR 指过去。"""
    models = tmp_path / "install" / "vcr" / "models"
    models.mkdir(parents=True)
    monkeypatch.setattr(config, "MODEL_DIR", str(models))
    monkeypatch.setattr(ovb, "_OV", None)
    monkeypatch.setattr(ovb, "_OV_ERROR", "")
    monkeypatch.setattr(ovb, "_OV_SOURCE", "")
    monkeypatch.setattr(ovb, "_DEVINFO", None)
    return tmp_path / "install"


def test_search_roots_cover_install_root_and_vcr_dir(install):
    roots = [os.path.normcase(r) for r in ovb._runtime_search_roots()]
    assert os.path.normcase(str(install / "vcr" / "models" / "_runtime")) == roots[0]
    # 用户实测放错的那一层
    assert os.path.normcase(str(install / "_runtime")) in roots
    assert os.path.normcase(str(install / "vcr" / "_runtime")) in roots


def test_zip_at_install_root_is_accepted_and_extracted_to_canonical(install):
    """用户把 zip 放安装根目录 → 仍要能用，且解压归位到模型目录。"""
    _make_zip(str(install / "_runtime" / ovb.ZIP_NAME))
    got = ovb.ensure_runtime_extracted()
    canonical = os.path.join(str(install), "vcr", "models", "_runtime", ovb.RT_DIR)
    assert os.path.normcase(got) == os.path.normcase(canonical)
    assert ovb._is_runtime_dir(got)
    # 规范位置没有 zip，也不该留半成品
    assert not os.path.exists(os.path.join(canonical + ".partial", "openvino", "x"))


def test_canonical_zip_still_works(install):
    _make_zip(str(install / "vcr" / "models" / "_runtime" / ovb.ZIP_NAME))
    got = ovb.ensure_runtime_extracted()
    assert ovb._is_runtime_dir(got)


def test_half_extracted_dir_is_not_treated_as_ready(install):
    """只剩 openvino/libs 的残骸不能算「已安装」——必须重新解压。"""
    ovrt = install / "vcr" / "models" / "_runtime" / ovb.RT_DIR
    (ovrt / "openvino" / "libs").mkdir(parents=True)
    (ovrt / "openvino" / "libs" / "openvino.dll").write_text("x")
    assert not ovb._is_runtime_dir(str(ovrt))
    assert ovb.ensure_runtime_extracted() == ""      # 没有 zip 可解 → 空串，不装作可用

    # 补上 zip 后应能自愈（旧残骸被替换）
    _make_zip(str(install / "_runtime" / ovb.ZIP_NAME))
    got = ovb.ensure_runtime_extracted()
    assert ovb._is_runtime_dir(got)
    assert os.path.isfile(os.path.join(got, "openvino", "__init__.py"))


def test_no_zip_error_message_names_the_exact_path(install):
    """找不到时的错误必须自带「该放哪」，否则又是静默失败。"""
    ov, err, src = ovb.load_runtime()
    assert ov is None and src == ""
    assert ovb.ZIP_NAME in err
    assert os.path.normcase(os.path.join(str(install), "vcr", "models", "_runtime")) in os.path.normcase(err)
