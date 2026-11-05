//! Os monitores da maquina, para transmitir a tela inteira.
//!
//! Ate aqui o app so sabia capturar JANELA, e tela inteira e o caso mais
//! comum de todos: e o que se usa para mostrar um jogo em tela cheia, que e
//! justamente onde a captura de janela costuma falhar.
//!
//! O Windows Graphics Capture aceita os dois (`CreateForWindow` e
//! `CreateForMonitor`), entao a diferenca entre os dois caminhos e so qual
//! item se cria: dali para frente o pipeline e o mesmo.

use windows::core::Result as WinResult;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
// A constante mora no modulo de janelas, e nao no de GDI, apesar de so ser
// usada com monitor.
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub hmonitor: isize,
    /// O nome do dispositivo, como `\\.\DISPLAY1`.
    pub dispositivo: String,
    pub largura: u32,
    pub altura: u32,
    pub principal: bool,
    /// Onde ele fica na area de trabalho. Serve para ordenar da esquerda para
    /// a direita, que e como a pessoa ve os monitores na mesa dela.
    pub esquerda: i32,
}

impl Monitor {
    /// O que aparece na lista: "Tela 1 (principal), 2560x1440".
    pub fn rotulo(&self, indice: usize) -> String {
        rotulo_de(indice, self.principal, self.largura, self.altura)
    }
}

/// Puro, para poder ser testado sem monitor nenhum.
pub fn rotulo_de(indice: usize, principal: bool, largura: u32, altura: u32) -> String {
    let marca = if principal { " (principal)" } else { "" };
    format!("Tela {}{}, {}x{}", indice + 1, marca, largura, altura)
}

unsafe extern "system" fn juntar(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    dados: LPARAM,
) -> BOOL {
    let lista = &mut *(dados.0 as *mut Vec<Monitor>);

    // MONITORINFOEXW e MONITORINFO mais o nome do dispositivo no fim. O
    // `cbSize` e quem diz ao Windows qual dos dois ele esta preenchendo: com o
    // tamanho errado o nome volta vazio e sem erro nenhum.
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
        return BOOL(1); // segue para o proximo: um monitor ilegivel nao para a lista
    }

    let r = info.monitorInfo.rcMonitor;
    let fim = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
    lista.push(Monitor {
        hmonitor: hmonitor.0 as isize,
        dispositivo: String::from_utf16_lossy(&info.szDevice[..fim]),
        largura: (r.right - r.left).unsigned_abs(),
        altura: (r.bottom - r.top).unsigned_abs(),
        principal: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        esquerda: r.left,
    });
    BOOL(1)
}

/// Avisa ao Windows que este processo entende tela com escala.
///
/// Sem isso o sistema MENTE para ele, por compatibilidade: devolve 1536x864
/// onde ha 1920x1080 a 125%, e a lista mostraria um tamanho que nao e o da
/// tela de ninguem. Pior, mentiria so as vezes: a janela do app liga a
/// consciencia de DPI sozinha, entao o mesmo executavel dizia um tamanho na
/// linha de comando e outro na interface.
///
/// Chamar duas vezes e inofensivo: a segunda falha e e isso.
pub fn ajustar_dpi() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}

/// Os monitores, do mais a esquerda para o mais a direita.
pub fn listar() -> WinResult<Vec<Monitor>> {
    let mut lista: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(juntar),
            LPARAM(&mut lista as *mut _ as isize),
        )
        .ok()?;
    }
    // A ordem da enumeracao nao segue a disposicao fisica. Ordenar pela
    // coordenada esquerda faz "Tela 1" ser a da esquerda, que e a que a pessoa
    // chama de primeira.
    lista.sort_by_key(|m| m.esquerda);
    Ok(lista)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn o_rotulo_conta_a_partir_de_um() {
        // Ninguem chama o primeiro monitor de "Tela 0".
        assert_eq!(rotulo_de(0, false, 1920, 1080), "Tela 1, 1920x1080");
        assert_eq!(rotulo_de(2, false, 1280, 720), "Tela 3, 1280x720");
    }

    #[test]
    fn o_principal_vem_marcado() {
        assert_eq!(rotulo_de(0, true, 2560, 1440), "Tela 1 (principal), 2560x1440");
    }
}
