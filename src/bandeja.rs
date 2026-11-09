//! O ícone ao lado do relógio, para o app sumir durante o jogo.
//!
//! É o motivo de a janela existir separada do núcleo: quem transmite está
//! jogando, e no momento que importa não deve haver interface nenhuma
//! desenhando. Fechada para a bandeja, a janela não pinta um pixel, e a
//! transmissão continua porque ela nunca morou ali.
//!
//! O menu é curto de propósito. **Não dá para içar a tela daqui**: içar exige
//! escolher o que transmitir, e escolher sem ver a lista é o jeito de mandar a
//! janela errada para o salão. Baixar, sim: é o que se quer fazer com pressa,
//! no meio de uma partida.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

/// O que a bandeja pede à janela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pedido {
    Mostrar,
    Baixar,
    Sair,
}

pub struct Bandeja {
    /// Guardado porque soltar o ícone o apaga da bandeja. Não é campo morto.
    _icone: TrayIcon,
    baixar: MenuItem,
    recebe: std::sync::mpsc::Receiver<Pedido>,
}

impl Bandeja {
    /// Monta a bandeja. Tem de ser chamada na thread da janela: é ela que
    /// bombeia as mensagens do Windows, e sem bomba o menu não abre.
    ///
    /// Devolve nada se o sistema recusar (acontece em sessão sem área de
    /// trabalho). Aí o app segue sem bandeja, que é melhor que não abrir.
    pub fn nova(ctx: &eframe::egui::Context) -> Option<Self> {
        let menu = Menu::new();
        let abrir = MenuItem::new("Abrir a janela", true, None);
        let baixar = MenuItem::new("Baixar a tela", false, None);
        let sair = MenuItem::new("Sair", true, None);
        menu.append(&abrir).ok()?;
        menu.append(&baixar).ok()?;
        menu.append(&PredefinedMenuItem::separator()).ok()?;
        menu.append(&sair).ok()?;

        let (manda, recebe) = std::sync::mpsc::channel();
        // O canal vai num cadeado porque o tratador precisa ser Sync, e um
        // remetente de canal não é. O cadeado nunca é disputado: quem fala
        // aqui é a thread das mensagens do Windows, uma só.
        let correio = std::sync::Mutex::new(manda.clone());
        let ids = (abrir.id().clone(), baixar.id().clone(), sair.id().clone());
        let acorda = ctx.clone();
        // Tratador em vez do canal pronto da biblioteca: ele é global, e
        // quando existe um tratador o canal dela não recebe mais nada. Além
        // disso é aqui que a janela precisa ser ACORDADA: escondida, ela não
        // desenha, e sem o pedido de redesenho o clique não chegaria a lugar
        // nenhum.
        MenuEvent::set_event_handler(Some(move |evento: MenuEvent| {
            let pedido = classificar(&evento.id, &ids);
            if let (Some(p), Ok(correio)) = (pedido, correio.lock()) {
                let _ = correio.send(p);
            }
            acorda.request_repaint();
        }));

        let correio = std::sync::Mutex::new(manda);
        let acorda = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |evento: TrayIconEvent| {
            // Clique simples no ícone traz a janela de volta, que é o que todo
            // mundo tenta primeiro.
            if let TrayIconEvent::Click { .. } = evento {
                if let Ok(correio) = correio.lock() {
                    let _ = correio.send(Pedido::Mostrar);
                }
            }
            acorda.request_repaint();
        }));

        let icone = tray_icon::Icon::from_rgba(crate::selo::icone(32), 32, 32).ok()?;
        let bandeja = TrayIconBuilder::new()
            .with_icon(icone)
            .with_tooltip("Tela do Boteco")
            .with_menu(Box::new(menu))
            // No Windows o menu é do botão direito. O esquerdo abre a janela.
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        Some(Self {
            _icone: bandeja,
            baixar,
            recebe,
        })
    }

    /// O que foi pedido desde a última olhada.
    pub fn pedidos(&self) -> Vec<Pedido> {
        self.recebe.try_iter().collect()
    }

    /// Só dá para baixar a tela quando há tela no ar. Item que não faz nada é
    /// pior que item que não existe: a pessoa clica e fica sem resposta.
    pub fn mostrar_que_esta_no_ar(&self, no_ar: bool) {
        self.baixar.set_enabled(no_ar);
    }
}

/// Qual pedido é aquele id de menu.
fn classificar(id: &MenuId, ids: &(MenuId, MenuId, MenuId)) -> Option<Pedido> {
    if id == &ids.0 {
        Some(Pedido::Mostrar)
    } else if id == &ids.1 {
        Some(Pedido::Baixar)
    } else if id == &ids.2 {
        Some(Pedido::Sair)
    } else {
        None
    }
}

/// O que o X da janela faz.
///
/// Transmitindo, ele **esconde**: a transmissão continua, e é exatamente para
/// isso que a bandeja existe. Parado, ele sai: aplicativo que não está fazendo
/// nada e fica morando na bandeja é aplicativo que ninguém pediu, ocupando
/// memória que o jogo quer.
pub fn fechar_e_sair(no_ar: bool) -> bool {
    !no_ar
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn fechar_transmitindo_apenas_esconde() {
        assert!(!fechar_e_sair(true));
    }

    #[test]
    fn fechar_parado_encerra_o_app() {
        // Senão ele fica morando na bandeja sem ninguém ter pedido.
        assert!(fechar_e_sair(false));
    }

    #[test]
    fn cada_item_do_menu_vira_o_seu_pedido() {
        let ids = (
            MenuId::new("abrir"),
            MenuId::new("baixar"),
            MenuId::new("sair"),
        );
        assert_eq!(classificar(&ids.0, &ids), Some(Pedido::Mostrar));
        assert_eq!(classificar(&ids.1, &ids), Some(Pedido::Baixar));
        assert_eq!(classificar(&ids.2, &ids), Some(Pedido::Sair));
    }

    #[test]
    fn id_de_fora_nao_vira_pedido_nenhum() {
        // O tratador é GLOBAL: se um dia houver outro menu no processo, os
        // eventos dele passam por aqui também. Chutar um pedido ali seria
        // sair do app por causa de um clique que não era nosso.
        let ids = (
            MenuId::new("abrir"),
            MenuId::new("baixar"),
            MenuId::new("sair"),
        );
        assert_eq!(classificar(&MenuId::new("outro"), &ids), None);
    }
}
