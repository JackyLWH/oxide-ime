$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
# 原圖的透明留白佔了近三分之一，縮成工作列小圖時會令標誌過細。
# 依可見像素計算裁切範圍，讓完整圖案盡量填滿 Windows 圖標位置；原始 PNG 不會改動。
if (-not ('OxideImeIconBounds' -as [type])) {
    $references = @(
        [System.Drawing.Bitmap].Assembly.Location,
        [System.Drawing.Color].Assembly.Location,
        [System.Runtime.GCSettings].Assembly.Location,
        [System.Object].Assembly.Location
    ) + @(Get-ChildItem -LiteralPath (Split-Path ([System.Drawing.Bitmap].Assembly.Location)) -Filter 'System.Private.Windows.*.dll' | ForEach-Object FullName)
    Add-Type -ReferencedAssemblies ($references | Select-Object -Unique) -TypeDefinition @'
using System;
using System.Drawing;
public static class OxideImeIconBounds {
    public static Rectangle Find(Bitmap image) {
        int left = image.Width, top = image.Height, right = -1, bottom = -1;
        for (int y = 0; y < image.Height; y++) {
            for (int x = 0; x < image.Width; x++) {
                if (image.GetPixel(x, y).A > 32) {
                    left = Math.Min(left, x); top = Math.Min(top, y);
                    right = Math.Max(right, x); bottom = Math.Max(bottom, y);
                }
            }
        }
        if (right < left) throw new InvalidOperationException("圖標沒有可見內容。");
        return Rectangle.FromLTRB(left, top, right + 1, bottom + 1);
    }
}
'@
}
$root = Split-Path $PSScriptRoot -Parent
$source = Join-Path $root 'assets\oxide-ime.png'
$target = Join-Path $root 'assets\oxide-ime.ico'
$image = [System.Drawing.Bitmap]::FromFile($source)
try {
    $content = [OxideImeIconBounds]::Find($image)
    $images = [System.Collections.Generic.List[object]]::new()
    foreach ($size in @(16, 20, 24, 32, 40, 48, 64, 128, 256)) {
        $bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $stream = [System.IO.MemoryStream]::new()
        try {
            $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.Clear([System.Drawing.Color]::Transparent)
            $scale = $size / [Math]::Max($content.Width, $content.Height)
            $width = [Math]::Max(1, [int][Math]::Round($content.Width * $scale))
            $height = [Math]::Max(1, [int][Math]::Round($content.Height * $scale))
            $destination = [System.Drawing.Rectangle]::new([int](($size - $width) / 2), [int](($size - $height) / 2), $width, $height)
            $graphics.DrawImage($image, $destination, $content.X, $content.Y, $content.Width, $content.Height, [System.Drawing.GraphicsUnit]::Pixel)
            $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
            $images.Add([PSCustomObject]@{ Size = $size; Bytes = $stream.ToArray() })
        } finally {
            $stream.Dispose()
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    }
    $file = [System.IO.File]::Create($target)
    $writer = [System.IO.BinaryWriter]::new($file)
    try {
        $writer.Write([uint16]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]$images.Count)
        $offset = 6 + 16 * $images.Count
        foreach ($entry in $images) {
            $dimension = if ($entry.Size -eq 256) { 0 } else { $entry.Size }
            $writer.Write([byte]$dimension)
            $writer.Write([byte]$dimension)
            $writer.Write([byte]0)
            $writer.Write([byte]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$entry.Bytes.Length)
            $writer.Write([uint32]$offset)
            $offset += $entry.Bytes.Length
        }
        foreach ($entry in $images) { $writer.Write([byte[]]$entry.Bytes) }
    } finally {
        $writer.Dispose()
        $file.Dispose()
    }
} finally {
    $image.Dispose()
}
Write-Host "已產生 $target"
