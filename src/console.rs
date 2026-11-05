//! O executável é de INTERFACE e ainda assim fala no terminal.
//!
//! Windows decide isso na hora de ligar o programa, pelo subsistema gravado no
//! arquivo, e os dois jeitos têm um defeito:
//!
//! - **Console**: quem dá dois cliques no app vê uma janela preta abrir junto,
//!   que não serve para nada e fica aberta atrás da interface.
//! - **Janela**: não há console nenhum, e aí `--listar`, `--vivo` e a medição
//!   do codificador escrevem para o nada. Essa é a ferramenta de diagnóstico
//!   deste app, e perdê-la para ganhar estética é troca ruim.
//!
//! Então o executável é de janela e, quando é chamado DE um terminal, se
//! pendura no console de quem o chamou. Dois cliques não têm console pai, a
//! chamada falha, e é só isso que acontece: ninguém vê janela preta.

/// Pendura a saída no console de quem chamou, se houver um.
pub fn anexar() {
    unsafe {
        // Falha é o caso normal (duplo clique não tem console pai), por isso o
        // erro é descartado sem avisar: não há onde avisar.
        let _ = windows::Win32::System::Console::AttachConsole(
            windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}
