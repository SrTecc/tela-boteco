# Prova que o X da janela faz o que tem de fazer.
#
#   powershell -File scripts/prova-da-bandeja.ps1
#
# São dois comportamentos opostos no mesmo botão, e nenhum dos dois aparece em
# teste de unidade, porque quem decide ali é o Windows mandando o fechamento e
# o eframe aceitando ou cancelando:
#
#   transmitindo -> some para a bandeja, e o processo CONTINUA vivo
#   parado       -> fecha o app
#
# Errar isso de um lado mata a transmissão de quem está jogando; do outro,
# deixa um aplicativo morando na bandeja sem ninguém ter pedido.

$codigo = @'
using System;
using System.Runtime.InteropServices;
public class Jan {
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  public const uint WM_CLOSE = 0x0010;
}
'@
if (-not ("Jan" -as [type])) { Add-Type -TypeDefinition $codigo }

Set-Location (Split-Path $PSScriptRoot -Parent)
$exe = ".\target\debug\tela-boteco.exe"
if (-not (Test-Path $exe)) { Write-Error "compila primeiro: cargo build"; exit 1 }

function Fechar-E-Olhar([string]$cena) {
  Get-Process tela-boteco -ErrorAction SilentlyContinue | Stop-Process
  Start-Sleep -Milliseconds 500
  $env:TELA_CENA = $cena
  Start-Process -FilePath $exe
  Start-Sleep -Seconds 4

  $p = Get-Process tela-boteco -ErrorAction SilentlyContinue
  if (-not $p) { return @{ cena = $cena; erro = "o app nem abriu" } }
  $janela = $p.MainWindowHandle
  if (-not [Jan]::IsWindowVisible($janela)) { return @{ cena = $cena; erro = "a janela nao estava visivel antes do teste" } }

  [Jan]::PostMessageW($janela, [Jan]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  Start-Sleep -Seconds 3

  $depois = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
  $resultado = @{
    cena = $cena
    vivo = [bool]$depois
    visivel = if ($depois) { [Jan]::IsWindowVisible($janela) } else { $false }
  }
  if ($depois) { $depois | Stop-Process }
  return $resultado
}

$falhas = 0

$r = Fechar-E-Olhar "no-ar"
if ($r.erro) { Write-Output "no-ar   FALHOU: $($r.erro)"; $falhas++ }
elseif ($r.vivo -and -not $r.visivel) { Write-Output "no-ar   ok: sumiu para a bandeja e continua vivo" }
else { Write-Output "no-ar   FALHOU: vivo=$($r.vivo) visivel=$($r.visivel) (devia estar vivo e escondido)"; $falhas++ }

$r = Fechar-E-Olhar "parado"
if ($r.erro) { Write-Output "parado  FALHOU: $($r.erro)"; $falhas++ }
elseif (-not $r.vivo) { Write-Output "parado  ok: o app fechou" }
else { Write-Output "parado  FALHOU: continuou vivo (devia ter fechado)"; $falhas++ }

Remove-Item Env:\TELA_CENA -ErrorAction SilentlyContinue
if ($falhas -gt 0) { exit 1 }
