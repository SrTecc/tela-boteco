# Fotografa a janela do app, para conferir a interface sem descrever de boca.
#
# Olhar a imagem é a única forma de dizer se o tema está certo: tamanho de
# fonte, cor, alinhamento e corte de texto não aparecem em teste nenhum.
#
#   powershell -File scripts/foto-da-janela.ps1 -Titulo "Tela do Boteco" -Saida foto.png
#
# Duas decisões que custaram tentativa:
#
# - **PrintWindow, e não CopyFromScreen.** Copiar da tela devolve o que está
#   composto ali, e a janela do app é desenhada pela placa: o que veio foi o
#   papel de parede, com a janela aberta e visível na frente. O
#   PW_RENDERFULLCONTENT (2) pede o conteúdo à própria janela.
# - **O script CONFERE o que fotografou.** Imagem de uma cor só é o jeito
#   silencioso de essa captura falhar, e aceitar isso calado faria eu aprovar
#   uma tela que nunca vi.

param(
  [string]$Titulo = "Tela do Boteco",
  [string]$Saida = "foto.png"
)

Add-Type -AssemblyName System.Drawing

$codigo = @'
using System;
using System.Runtime.InteropServices;
public class Janela {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Retangulo r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint opcoes);
  [StructLayout(LayoutKind.Sequential)]
  public struct Retangulo { public int Esquerda, Topo, Direita, Base; }
}
'@
if (-not ("Janela" -as [type])) { Add-Type -TypeDefinition $codigo }

$processo = Get-Process | Where-Object { $_.MainWindowTitle -eq $Titulo } | Select-Object -First 1
if (-not $processo) {
  Write-Error "nao achei janela com o titulo '$Titulo' (o app esta aberto?)"
  exit 1
}

[Janela]::SetForegroundWindow($processo.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 400

$r = New-Object Janela+Retangulo
[Janela]::GetWindowRect($processo.MainWindowHandle, [ref]$r) | Out-Null
$largura = $r.Direita - $r.Esquerda
$altura = $r.Base - $r.Topo
if ($largura -le 0 -or $altura -le 0) {
  Write-Error "a janela devolveu tamanho $largura x $altura"
  exit 1
}

$imagem = New-Object System.Drawing.Bitmap $largura, $altura
$tela = [System.Drawing.Graphics]::FromImage($imagem)
$dc = $tela.GetHdc()
$deu = [Janela]::PrintWindow($processo.MainWindowHandle, $dc, 2)
$tela.ReleaseHdc($dc)
if (-not $deu) {
  # Último recurso: o que estiver composto na tela naquele retângulo.
  $tela.CopyFromScreen($r.Esquerda, $r.Topo, 0, 0, $imagem.Size)
}

# Confere: uma cor só quer dizer que a captura falhou sem avisar.
$cores = @{}
for ($x = 4; $x -lt $largura; $x += 17) {
  for ($y = 4; $y -lt $altura; $y += 17) {
    $cores[$imagem.GetPixel($x, $y).ToArgb()] = $true
  }
}

$imagem.Save($Saida, [System.Drawing.Imaging.ImageFormat]::Png)
$tela.Dispose()
$imagem.Dispose()

Write-Output "$Saida  $largura x $altura  $($cores.Count) cores"
if ($cores.Count -lt 3) {
  Write-Error "a foto saiu de uma cor so: a captura nao pegou o conteudo da janela"
  exit 2
}
