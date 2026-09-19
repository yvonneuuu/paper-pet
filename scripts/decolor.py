"""把终端日志里的 ANSI 转义码去掉，方便直接读。"""
import io
import os
import re
import sys

path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.environ["LOCALAPPDATA"], "Temp", "tauri-dev.log"
)

text = io.open(path, encoding="utf-8", errors="replace").read()
text = re.sub("\x1b\\[[0-9;]*m", "", text)          # 颜色
text = re.sub("\x1b\\]8;;[^\x07\x1b]*(\x07|\x1b\\\\)", "", text)  # 超链接
print(text)
