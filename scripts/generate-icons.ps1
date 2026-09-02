# 从 ico.png (512x512) 统一生成所有图标
# 覆盖 src-tauri/icons 目录内所有 PNG + 重新生成 icon.ico（多尺寸）
# 源图位于 src-tauri\icons\ico.png（用 $PSScriptRoot 定位，不依赖本机绝对路径）
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$scriptDir = $PSScriptRoot
if (-not $scriptDir) {
    # 兼容从其他目录调用（.ps1 直接执行时 $PSScriptRoot 恒非空，此处仅兜底）
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$iconsDir = Join-Path $scriptDir "..\src-tauri\icons"
$src = Join-Path $iconsDir "ico.png"
$icon = [System.Drawing.Image]::FromFile($src)
if ($icon.Width -ne $icon.Height) {
    Write-Error "源图不是正方形: $($icon.Width)x$($icon.Height)"
    exit 1
}

# 目标尺寸表：文件名 -> 尺寸（仅含实际使用的图标：tauri.conf bundle.icon 引用项 + 托盘 32）
# 注意：Square*Logo / StoreLogo / icon.icns 为 MSIX/macOS 专用，本项目仅 NSIS(Windows)，
# 不生成（已从仓库清理，见 v0.3.31 代码审计）。
$targets = @{
    "32x32.png"          = 32
    "128x128.png"        = 128
    "128x128@2x.png"     = 256
    "icon.png"           = 512
}

function Resize-Image([System.Drawing.Image]$img, [int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.DrawImage($img, 0, 0, $size, $size)
    $g.Dispose()
    return $bmp
}

foreach ($name in $targets.Keys) {
    $size = $targets[$name]
    $bmp = Resize-Image $icon $size
    $out = Join-Path $iconsDir $name
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "生成 $name ($size x $size)"
}

# 生成 icon.ico：多尺寸 ICO（16/24/32/48/64/128/256，与当前产物一致）
$icoPath = Join-Path $iconsDir "icon.ico"
# 注意：Vista+ 支持 PNG 压缩 ICO，但 CreateIconFromResourceEx 对 PNG 压缩帧
# 会返回 ERROR_INVALID_HANDLE（见 commands/dsh.rs 注释取证）——本脚本仍输出 PNG 压缩
# ICO 供 exe 资源/tauri 打包使用；运行期窗口图标走 RGBA→CreateIcon 路径，不依赖本 ICO。
$sizes = @(16, 24, 32, 48, 64, 128, 256)
$images = @()
foreach ($s in $sizes) {
    $images += ,(Resize-Image $icon $s)
}
# 用 Icon.FromHandle 组装 ICO
$stream = New-Object System.IO.MemoryStream
# ICO 头部
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([uint16]0)          # reserved
$writer.Write([uint16]1)          # type: icon
$writer.Write([uint16]$images.Count)  # count
$offset = 6 + 16 * $images.Count
$pngDatas = @()
foreach ($img in $images) {
    # 将 PNG 编码（Vista+ 支持 PNG 压缩 ICO）
    $ms = New-Object System.IO.MemoryStream
    $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngDatas += ,$ms.ToArray()
    $ms.Dispose()
}
for ($i = 0; $i -lt $images.Count; $i++) {
    $s = $images[$i].Width
    $bytes = $pngDatas[$i]
    # 目录项
    $writer.Write([byte]($s -band 0xFF))   # width (0=256)
    $writer.Write([byte]($s -band 0xFF))   # height
    $writer.Write([byte]0)                 # colors
    $writer.Write([byte]0)                 # reserved
    $writer.Write([uint16]1)               # planes
    $writer.Write([uint16]32)              # bitcount
    $writer.Write([uint32]$bytes.Length)   # size
    $writer.Write([uint32]$offset)         # offset
    $offset += $bytes.Length
}
foreach ($d in $pngDatas) {
    $writer.Write($d)
}
$writer.Flush()
[System.IO.File]::WriteAllBytes($icoPath, $stream.ToArray())
$writer.Dispose()
$stream.Dispose()
foreach ($img in $images) { $img.Dispose() }
Write-Host "生成 icon.ico（含 $($sizes -join ',') px 多尺寸 PNG 压缩）"

$icon.Dispose()
Write-Host "ALL DONE"
