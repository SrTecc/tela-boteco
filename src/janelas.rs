//! A lista de janelas que a pessoa pode transmitir.
//!
//! É o primeiro pedaço do app e não é um detalhe de interface: a janela
//! escolhida entrega o **processo dono**, e é dele que o áudio vai ser
//! capturado. Escolher a janela e escolher a fonte de som são a mesma ação,
//! como no Discord.
//!
//! O que entra na lista é o que a pessoa reconheceria olhando a barra de
//! tarefas. O Windows tem centenas de janelas de verdade a qualquer momento, e
//! quase todas são invisíveis, de serviço, ou cascas vazias de aplicativos que
//! desenham noutro lugar.

use std::ffi::c_void;
use std::path::Path;
use windows::core::PWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, MAX_PATH, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GA_ROOTOWNER,
    GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

/// Uma janela candidata a ser transmitida.
#[derive(Debug, Clone)]
pub struct Janela {
    pub hwnd: isize,
    pub titulo: String,
    /// O processo dono. É o que o áudio por processo vai usar.
    pub pid: u32,
    pub executavel: String,
    pub largura: i32,
    pub altura: i32,
    /// Minimizada entra na lista, mas marcada. Sumir com ela é pior: a pessoa
    /// procura o aplicativo que ela sabe que está aberto, não acha, e conclui
    /// que o app é que está quebrado. Marcada, a interface pede para restaurar.
    pub minimizada: bool,
}

/// Janela pequena demais para ser conteúdo de verdade. O valor é generoso:
/// serve para cortar casca de 1x1 pixel, não para julgar o que alguém quer
/// transmitir.
const MINIMO: i32 = 120;

/// Peças do Windows que têm janela com título e nunca são conteúdo de ninguém.
/// Lista curta de propósito: filtrar por nome é frágil, e só vale para o que
/// aparece em toda máquina e confunde todo mundo.
const NUNCA: &[&str] = &["TextInputHost.exe", "StartMenuExperienceHost.exe", "SearchHost.exe"];

/// As janelas que valem aparecer na escolha, da maior para a menor.
pub fn listar() -> Vec<Janela> {
    let mut achadas: Vec<Janela> = Vec::new();
    let ponteiro = &mut achadas as *mut Vec<Janela> as isize;

    // `EnumWindows` chama de volta para cada janela de topo. O `unsafe` é
    // inerente: é uma API em C que recebe um ponteiro nosso de volta.
    unsafe {
        let _ = EnumWindows(Some(visitar), LPARAM(ponteiro));
    }

    achadas.sort_by(|a, b| (b.largura * b.altura).cmp(&(a.largura * a.altura)));
    achadas
}

extern "system" fn visitar(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let lista = unsafe { &mut *(lparam.0 as *mut Vec<Janela>) };
    if let Some(j) = examinar(hwnd) {
        lista.push(j);
    }
    TRUE // segue enumerando
}

/// Decide se uma janela entra na lista, e monta a ficha dela.
fn examinar(hwnd: HWND) -> Option<Janela> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }

        // Janela de ferramenta (barra flutuante, dica) nunca é conteúdo.
        let estilo = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if estilo & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }

        // Só a janela "raiz" de cada aplicativo: sem isso, um programa com
        // diálogos abertos aparece três ou quatro vezes na lista.
        if GetAncestor(hwnd, GA_ROOTOWNER) != hwnd {
            return None;
        }

        let tamanho = GetWindowTextLengthW(hwnd);
        if tamanho <= 0 {
            return None; // sem título não há como a pessoa reconhecer
        }
        let mut buffer = vec![0u16; tamanho as usize + 1];
        let lidos = GetWindowTextW(hwnd, &mut buffer);
        let titulo = String::from_utf16_lossy(&buffer[..lidos as usize]);
        if titulo.trim().is_empty() {
            return None;
        }

        // Encoberta pelo gerenciador de janelas: é o caso do aplicativo da
        // loja que ficou suspenso. Ele responde que está visível, tem título e
        // tem tamanho, mas não desenha nada. É isto que fazia "Configurações"
        // aparecer duas vezes, uma pelo quadro e outra pelo conteúdo.
        if encoberta(hwnd) {
            return None;
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let executavel = executavel_de(pid).unwrap_or_else(|| "?".into());
        if NUNCA.iter().any(|n| n.eq_ignore_ascii_case(&executavel)) {
            return None;
        }

        let minimizada = IsIconic(hwnd).as_bool();

        let mut retangulo = RECT::default();
        if GetWindowRect(hwnd, &mut retangulo).is_err() {
            return None;
        }
        let largura = retangulo.right - retangulo.left;
        let altura = retangulo.bottom - retangulo.top;
        // Janela minimizada mente o tamanho (o Windows a joga para fora da
        // tela), então o corte por tamanho não se aplica a ela.
        if !minimizada && (largura < MINIMO || altura < MINIMO) {
            return None;
        }

        Some(Janela {
            hwnd: hwnd.0 as isize,
            titulo,
            pid,
            executavel,
            largura,
            altura,
            minimizada,
        })
    }
}

/// A janela está encoberta (cloaked) pelo gerenciador de janelas?
fn encoberta(hwnd: HWND) -> bool {
    unsafe {
        let mut valor: u32 = 0;
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut valor as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        );
        // Falhou a pergunta? Trata como visível: na dúvida, mostrar a janela é
        // melhor que escondê-la de quem quer transmiti-la.
        ok.is_ok() && valor != 0
    }
}

/// O nome do executável de um processo.
///
/// Usa `QueryFullProcessImageNameW` com `PROCESS_QUERY_LIMITED_INFORMATION`,
/// que é o direito mínimo e funciona sem ser administrador. Processo de sistema
/// ainda pode recusar, e aí a janela entra na lista sem o nome em vez de sumir
/// dela.
fn executavel_de(pid: u32) -> Option<String> {
    unsafe {
        let processo = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; MAX_PATH as usize];
        let mut tamanho = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            processo,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut tamanho,
        );
        let _ = windows::Win32::Foundation::CloseHandle(processo);
        if ok.is_err() {
            return None;
        }
        let caminho = String::from_utf16_lossy(&buffer[..tamanho as usize]);
        Path::new(&caminho)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }
}
