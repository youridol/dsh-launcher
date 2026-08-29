# 从 ico.png (512x512) 统一生成所有图标
# 覆盖 icons 目录内所有 PNG + 重新生成 icon.ico（多尺寸）
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$src = "Y:\dsh-launcher\src-tauri\icons\ico.png"
$icon = [System.Drawing.Image]::FromFile($src)
if ($icon.Width -ne $icon.Height) {
    Write-Error "源图不是正方形: $($icon.Width)x$($icon.Height)"
    exit 1
}

# 目标尺寸表：文件名 -> 尺寸
$targets = @{
    "32x32.png"          = 32
    "128x128.png"        = 128
    "128x128@2x.png"     = 256
    "Square30x30Logo.png"   = 30
    "Square44x44Logo.png"   = 44
    "Square71x71Logo.png"   = 71
    "Square89x89Logo.png"   = 89
    "Square107x107Logo.png" = 107
    "Square142x142Logo.png" = 142
    "Square150x150Logo.png" = 150
    "Square284x284Logo.png" = 284
    "Square310x310Logo.png" = 310
    "StoreLogo.png"       = 50
    "icon.png"            = 512
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
    $out = Join-Path "Y:\dsh-launcher\src-tauri\icons" $name
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "生成 $name ($size x $size)"
}

# 生成 icon.ico：多尺寸 ICO（16/32/48/256）
$icoPath = "Y:\dsh-launcher\src-tauri\icons\icon.ico"
$sizes = @(16, 32, 48, 256)
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
$stream.Dispose()
foreach ($img in $images) { $img.Dispose() }
Write-Host "生成 icon.ico（含 $($sizes -join ',') px 多尺寸 PNG 压缩）"

$icon.Dispose()
Write-Host "ALL DONE"
