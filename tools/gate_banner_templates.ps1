# Draws the gate banner templates: all_18.png (18-stall gate) and all_9.png
# (9-stall gate), at the banner's default shape: the header's length (11.98 /
# 5.98 game units) by 2.5x its height (0.25 -> 0.625), ~19.2:1 and ~9.6:1,
# both 250 px high.
#
#   powershell -ExecutionPolicy Bypass -File tools\gate_banner_templates.ps1 [-Out docs\gate_banner\templates]
#
# The guides are approximate: the stall lines divide the header evenly, the
# safe area keeps text away from the plate's edges.

param([string]$Out = (Join-Path $PSScriptRoot "..\docs\gate_banner\templates"))

Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $Out | Out-Null

function Draw-Template([int]$Stalls, [int]$Width, [int]$Height, [string]$Ratio, [string]$Path) {
    $bmp = New-Object System.Drawing.Bitmap $Width, $Height, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

    # Background: two-tone stall cells so the gate's rhythm shows in game.
    $dark = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 24, 38, 58))
    $light = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 32, 50, 74))
    $cell = $Width / $Stalls
    for ($i = 0; $i -lt $Stalls; $i++) {
        $brush = if ($i % 2 -eq 0) { $dark } else { $light }
        $g.FillRectangle($brush, [single]($i * $cell), 0, [single]$cell + 1, $Height)
    }

    # Safe area: 10% inset top and bottom, 1% at the ends.
    $guide = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 90, 160, 220)), 2
    $guide.DashStyle = [System.Drawing.Drawing2D.DashStyle]::Dash
    $insetY = [single]($Height * 0.10)
    $insetX = [single]($Width * 0.01)
    $g.DrawRectangle($guide, $insetX, $insetY, [single]($Width - 2 * $insetX), [single]($Height - 2 * $insetY))

    # Centre marks (top and bottom, clear of the title).
    $centre = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 255, 200, 40)), 4
    $cx = [single]($Width / 2)
    $g.DrawLine($centre, $cx, 0, $cx, [single]($Height * 0.22))
    $g.DrawLine($centre, $cx, [single]($Height * 0.78), $cx, [single]$Height)

    # Border.
    $border = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 255, 255, 255)), 4
    $g.DrawRectangle($border, 2, 2, $Width - 4, $Height - 4)

    $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
    $grey = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 190, 205, 225))
    $title = New-Object System.Drawing.Font "Segoe UI", 64, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $small = New-Object System.Drawing.Font "Segoe UI", 44, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $mid = New-Object System.Drawing.StringFormat
    $mid.Alignment = [System.Drawing.StringAlignment]::Center
    $mid.LineAlignment = [System.Drawing.StringAlignment]::Center
    $left = New-Object System.Drawing.StringFormat
    $left.LineAlignment = [System.Drawing.StringAlignment]::Center
    $right = New-Object System.Drawing.StringFormat
    $right.Alignment = [System.Drawing.StringAlignment]::Far
    $right.LineAlignment = [System.Drawing.StringAlignment]::Center

    $text = "all_$Stalls.png  -  $Stalls-STALL GATE  -  ${Width} x ${Height} px ($Ratio)"
    $g.DrawString($text, $title, $white, (New-Object System.Drawing.RectangleF 0, 0, $Width, $Height), $mid)
    $g.DrawString("<< LEFT END", $small, $grey, (New-Object System.Drawing.RectangleF ($insetX + 12), 0, 600, $Height), $left)
    $g.DrawString("RIGHT END >>", $small, $grey, (New-Object System.Drawing.RectangleF 0, 0, ($Width - $insetX - 12), $Height), $right)

    $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "wrote $Path"
}

Draw-Template -Stalls 18 -Width 4800 -Height 250 -Ratio "19.2:1" -Path (Join-Path $Out "all_18.png")
Draw-Template -Stalls 9 -Width 2400 -Height 250 -Ratio "9.6:1" -Path (Join-Path $Out "all_9.png")
