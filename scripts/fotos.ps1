# Fotografa a interface em todas as cenas, para conferir antes de aprovar.
#
#   powershell -File scripts/fotos.ps1 -Pasta fotos
#
# Não é a tela sendo fotografada: é o próprio app desenhando num arquivo
# (`EFRAME_SCREENSHOT_TO`), porque a janela é desenhada pela placa e nem o
# `PrintWindow` pega o conteúdo dela — vem o quadro da janela com o miolo
# branco.
#
# As cenas existem porque a vida real não cabe num teste: ver a tela "no ar"
# exigiria duas pessoas numa call do servidor, e a tela que ninguém olha antes
# é a que sai torta.

param(
  [string]$Pasta = "fotos"
)

$cenas = @("fora", "parado", "no-ar", "no-ar-com-erro", "tomada", "fora-da-call")

New-Item -ItemType Directory -Force -Path $Pasta | Out-Null
Set-Location (Split-Path $PSScriptRoot -Parent)

foreach ($cena in $cenas) {
  $arquivo = Join-Path (Resolve-Path $Pasta) "$cena.png"
  if (Test-Path $arquivo) { Remove-Item $arquivo }
  $env:TELA_CENA = $cena
  $env:EFRAME_SCREENSHOT_TO = $arquivo
  cargo run --quiet --features eframe/__screenshot
  if (Test-Path $arquivo) {
    $tamanho = (Get-Item $arquivo).Length
    Write-Output ("{0,-14} {1}  ({2:N0} bytes)" -f $cena, $arquivo, $tamanho)
  } else {
    Write-Output ("{0,-14} NAO SAIU" -f $cena)
  }
}

Remove-Item Env:\TELA_CENA
Remove-Item Env:\EFRAME_SCREENSHOT_TO
