# Regenerates the Flipper Zero 1-bit rasters from the canonical sprite in src/render/sprite.rs.
Add-Type -AssemblyName System.Drawing
$out = $PSScriptRoot
$spritePath = Join-Path $PSScriptRoot "..\..\src\render\sprite.rs"

# Pull the BASE table out of sprite.rs so the raster can never drift from the pet's own art.
$rows = @()
$inBase = $false
foreach ($line in Get-Content $spritePath) {
  if ($line -match '^const BASE:') { $inBase = $true; continue }
  if ($inBase) {
    if ($line -match '^\];') { break }
    if ($line -match '^\s*"([.A-Za-z]{32})"') { $rows += $Matches[1] }
  }
}
if ($rows.Count -ne 32) { throw "expected 32 sprite rows, parsed $($rows.Count)" }

# Ink/paper by structure, not luminance: keep the visor slit, chest patch and tail rings light.
$ink = 'KMPHGJNALh'.ToCharArray()
$N = 32
$bits = New-Object 'bool[][]' $N
for ($y = 0; $y -lt $N; $y++) {
  $bits[$y] = New-Object 'bool[]' $N
  for ($x = 0; $x -lt $N; $x++) { $bits[$y][$x] = ($ink -ccontains $rows[$y][$x]) }  # -ccontains: 'G' must not match 'g'
}

function Save-Png($bits, $scale, $path) {
  $S = 32 * $scale
  $bmp = New-Object System.Drawing.Bitmap($S, $S, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp); $g.Clear([System.Drawing.Color]::White); $g.Dispose()
  for ($y = 0; $y -lt 32; $y++) { for ($x = 0; $x -lt 32; $x++) {
    if ($bits[$y][$x]) { for ($dy = 0; $dy -lt $scale; $dy++) { for ($dx = 0; $dx -lt $scale; $dx++) {
      $bmp.SetPixel($x * $scale + $dx, $y * $scale + $dy, [System.Drawing.Color]::Black) } } }
  } }
  $one = $bmp.Clone((New-Object System.Drawing.Rectangle(0, 0, $S, $S)), [System.Drawing.Imaging.PixelFormat]::Format1bppIndexed)
  $one.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $one.Dispose(); $bmp.Dispose()
  "{0}  ({1}x{1})" -f (Split-Path $path -Leaf), $S
}

Save-Png $bits 1 (Join-Path $out "raccy_32x32.png")
Save-Png $bits 2 (Join-Path $out "raccy_64x64.png")

# XBM: LSB-first within each byte, 4 bytes per 32 px row.
$bytes = @()
for ($y = 0; $y -lt 32; $y++) { for ($b = 0; $b -lt 4; $b++) {
  $v = 0
  for ($i = 0; $i -lt 8; $i++) { if ($bits[$y][$b * 8 + $i]) { $v = $v -bor (1 -shl $i) } }
  $bytes += $v
} }
$hex = $bytes | ForEach-Object { "0x{0:x2}" -f $_ }
$lines = for ($i = 0; $i -lt $hex.Count; $i += 12) {
  "    " + (($hex[$i..([Math]::Min($i + 11, $hex.Count - 1))]) -join ", ") + ","
}
@"
/* Raccy - 32x32 1-bit, generated from BASE in src/render/sprite.rs */
#define raccy_width 32
#define raccy_height 32
static const unsigned char raccy_bits[] = {
$($lines -join "`n")
};
"@ | Set-Content -Path (Join-Path $out "raccy.xbm") -Encoding ascii
"raccy.xbm  ($($bytes.Count) bytes)"
