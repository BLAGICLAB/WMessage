#!/usr/bin/env bash
# fetch_ocr_models.sh — 下载 wmessage Windows OCR（ocr_image 工具）所需的 PP-OCRv6 small ONNX 模型。
#
# 下载到仓库根 pp-ocr-v6/（模型已随仓库提交，本脚本仅用于更新/补齐；目录解析约定：
# WMESSAGE_OCR_MODEL_DIR 环境变量 → exe 同目录 pp-ocr-v6/ → 仓库根 pp-ocr-v6/）。
# 打包 Windows 绿色包时把整个 pp-ocr-v6/ 拷到 exe 同目录即可。
#
# 模型来源：RapidAI/RapidOCR 公开发布（PP-OCRv6 为 PaddlePaddle 官方模型，Apache 2.0）
#   ModelScope（主，国内可达）：https://www.modelscope.cn/models/RapidAI/RapidOCR
#   HuggingFace（镜像回退）：    https://huggingface.co/RapidAI/RapidOCR
#     （同一仓库同路径布局；本机网络无法直连 HF 未实测，ModelScope 全部 URL 已用
#      HTTP 206 ranged 请求实测有效，2026-09-10）
# 官方模型库：ModelScope collection PaddlePaddle/PP-OCRv6
#
# 固定 small 档（RapidOCR 3.9.x 同款默认）：
#   det  9.5MB  onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx
#   rec  20.2MB onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx
#   dict 75KB   paddle/PP-OCRv6/rec/PP-OCRv6_rec_small/ppocrv6_dict.txt
#               （v6 统一 50 语言：简中/繁中/英/日 + 46 拉丁语系；每行一个字符，
#                CTC 索引从 1 开始，0 为 blank）
#   cls  1.0MB  onnx/PP-OCRv5/cls/ch_PP-LCNet_x0_25_textline_ori_cls_mobile.onnx
#               （v6 无独立 cls 模型，沿用 v5 文本行方向分类；可选件）
#   合计 ≈ 31MB
#
# 用法：
#   bash scripts/fetch_ocr_models.sh                 # det + rec + dict + cls
#   WITH_CLS=0 bash scripts/fetch_ocr_models.sh      # 跳过方向分类模型

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${1:-$SCRIPT_DIR/../pp-ocr-v6}"
mkdir -p "$DEST"

WITH_CLS="${WITH_CLS:-1}"            # 1=下载 cls.onnx（方向分类，可选）

MS_BASE="https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master"
HF_BASE="https://huggingface.co/RapidAI/RapidOCR/resolve/main"

# curl_fetch <远端相对路径> <本地文件名>
# ModelScope 主源失败时回退 HuggingFace 镜像（同一 RapidAI/RapidOCR 仓库）。
# 完整性强约束：先写 $out.partial，校验大小下限 + 首字节非 HTML 后才 mv 就位——
# 截断文件 / CDN 错误页不会被 -s 当成「已存在」永久跳过。
curl_fetch() {
  local rel="$1" out="$2"
  if [ -s "$DEST/$out" ]; then
    echo "已存在，跳过：$out"
    return 0
  fi
  local tmp="$DEST/$out.partial"
  rm -f "$tmp"
  echo "下载 $out ← $rel"
  if ! curl -fL --retry 3 -o "$tmp" "$MS_BASE/$rel"; then
    echo "ModelScope 失败，回退 HuggingFace：$rel"
    if ! curl -fL --retry 3 -o "$tmp" "$HF_BASE/$rel"; then
      rm -f "$tmp"
      return 1
    fi
  fi
  # 校验：大小下限（ONNX ≥ 1MB，字典 ≥ 10KB，远低于标称值不误伤）+ 首字节非 '<'（HTML 错误页）
  local min_size=1000000
  case "$out" in keys.txt) min_size=10000 ;; esac
  local size
  size=$(wc -c < "$tmp" | tr -d ' ')
  if [ "$size" -lt "$min_size" ]; then
    rm -f "$tmp"
    echo "✗ $out 仅 $size 字节（< 下限 $min_size），疑似截断/错误页，请重跑" >&2
    return 1
  fi
  if [ "$(head -c 1 "$tmp")" = "<" ]; then
    rm -f "$tmp"
    echo "✗ $out 以 '<' 开头，疑似 HTML 错误页而非模型，请重跑" >&2
    return 1
  fi
  mv "$tmp" "$DEST/$out"
}

# det：检测（PP-OCRv6 small，9.5MB）
curl_fetch "onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx" "det.onnx"

# rec：识别（PP-OCRv6 small，20.2MB）
curl_fetch "onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx" "rec.onnx"

# keys：CTC 字典（ppocrv6_dict.txt，75KB）
curl_fetch "paddle/PP-OCRv6/rec/PP-OCRv6_rec_small/ppocrv6_dict.txt" "keys.txt"

# cls：方向分类（0°/180°），可选；v6 无独立 cls，沿用 PP-OCRv5 mobile（1.0MB）
if [ "$WITH_CLS" = "1" ]; then
  curl_fetch "onnx/PP-OCRv5/cls/ch_PP-LCNet_x0_25_textline_ori_cls_mobile.onnx" "cls.onnx"
fi

echo
echo "完成。模型目录：$DEST"
ls -lh "$DEST"
