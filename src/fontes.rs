//! O que dá para transmitir: as janelas e os monitores, numa lista só.
//!
//! A lista é montada **aqui**, e não na interface nem na linha de comando,
//! porque as duas precisam exatamente da mesma: o que a janela mostra tem de
//! ser o que o terminal resolve, senão a pessoa escolhe "tela 2" na interface
//! e recebe outra coisa no `--transmitir-sala`.
//!
//! A regra de ESCOLHA por texto continua pura e sobre rótulos, não sobre
//! handles do Windows: dá para testá-la sem monitor nenhum e sem janela
//! nenhuma, numa máquina de teste que nem tem tela.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escolha {
    Janela(usize),
    Monitor(usize),
}

/// Uma coisa que dá para transmitir, já com o som que vai junto.
#[derive(Clone, Debug)]
pub struct Fonte {
    /// O nome que aparece na lista.
    pub rotulo: String,
    /// A segunda linha, miúda: o executável e o tamanho, ou o dispositivo.
    pub detalhe: String,
    /// Onde a busca por texto procura. Rótulo e detalhe juntos, porque quem
    /// digita pensa no aplicativo ("code") e o título costuma ser outra coisa.
    pub busca: String,
    pub alvo: crate::captura::Alvo,
    pub som: crate::audio::Som,
    pub tela_inteira: bool,
    /// Minimizada aparece na lista, mas marcada: sumir com ela é pior, porque
    /// a pessoa procura o aplicativo que sabe que está aberto, não acha, e
    /// conclui que o app é que está quebrado.
    pub minimizada: bool,
}

/// Tudo que dá para transmitir: os monitores primeiro, as janelas depois.
///
/// Monitor na frente de propósito: "a tela inteira" é a escolha mais comum, e
/// é a que não falha quando o jogo está em tela cheia sem janela capturável.
pub fn listar() -> Vec<Fonte> {
    let janelas = crate::janelas::listar();
    let monitores = crate::monitores::listar().unwrap_or_default();

    // O Discord aparece na lista de janelas, então achar o processo dele não
    // custa nenhuma API nova.
    let discord = janelas
        .iter()
        .find(|j| j.executavel.eq_ignore_ascii_case("Discord.exe"))
        .map(|j| j.pid);

    let mut fontes: Vec<Fonte> = Vec::with_capacity(monitores.len() + janelas.len());

    for (i, m) in monitores.iter().enumerate() {
        let rotulo = m.rotulo(i);
        fontes.push(Fonte {
            busca: format!("{rotulo} {}", m.dispositivo),
            rotulo,
            detalhe: m.dispositivo.clone(),
            alvo: crate::captura::Alvo::Monitor(m.hmonitor),
            som: som_do_monitor(discord),
            tela_inteira: true,
            minimizada: false,
        });
    }

    for j in &janelas {
        let detalhe = if j.minimizada {
            format!("{} · minimizada", j.executavel)
        } else {
            format!("{} · {}x{}", j.executavel, j.largura, j.altura)
        };
        fontes.push(Fonte {
            busca: format!("{} {}", j.titulo, j.executavel),
            rotulo: j.titulo.clone(),
            detalhe,
            alvo: crate::captura::Alvo::Janela(j.hwnd),
            som: crate::audio::Som::Processo(j.pid),
            tela_inteira: false,
            minimizada: j.minimizada,
        });
    }

    fontes
}

/// O som que acompanha a tela inteira: tudo **menos** a call.
///
/// Monitor com som inteiro faria quem assiste ouvir a própria voz com atraso,
/// e o eco fica insuportável em segundos. Sem o Discord na máquina não há o
/// que excluir, e aí vai sem som: imagem muda é melhor que eco.
pub fn som_do_monitor(pid_do_discord: Option<u32>) -> crate::audio::Som {
    match pid_do_discord {
        Some(pid) => crate::audio::Som::TudoMenos(pid),
        None => crate::audio::Som::Nenhum,
    }
}

/// Onde a escolha por texto cai, na lista única do `listar`.
///
/// Existe porque a lista junta os dois tipos e a escolha conta cada um por si:
/// é uma tradução de índice, e tradução de índice errada aponta para a fonte
/// vizinha sem dar erro nenhum.
pub fn indice_na_lista(escolha: Escolha, quantos_monitores: usize) -> usize {
    match escolha {
        Escolha::Monitor(i) => i,
        Escolha::Janela(i) => quantos_monitores + i,
    }
}

/// Quem o texto escolhe, entre as janelas e os monitores.
///
/// "tela", "tela 2", "monitor 2" e "tela2" apontam para monitor; qualquer
/// outra coisa é procurada como pedaço do nome de uma janela. Sozinho, "tela"
/// vale o monitor principal, porque é o que a pessoa quer dizer quando não
/// especifica.
///
/// Os rótulos de monitor são os do `monitores::rotulo_de`, e é deles que sai
/// qual é o principal.
pub fn escolher(texto: &str, janelas: &[String], monitores: &[String]) -> Option<Escolha> {
    let t = texto.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }

    if let Some(resto) = t
        .strip_prefix("tela")
        .or_else(|| t.strip_prefix("monitor"))
        .or_else(|| t.strip_prefix("display"))
    {
        let resto = resto.trim();
        if resto.is_empty() {
            // Sem número: o principal, e na falta dele o primeiro da lista.
            let principal = monitores
                .iter()
                .position(|r| r.to_lowercase().contains("principal"))
                .unwrap_or(0);
            return (!monitores.is_empty()).then_some(Escolha::Monitor(principal));
        }
        if let Ok(n) = resto.parse::<usize>() {
            // Conta a partir de 1, como o rótulo mostra. O `if` em vez de um
            // `then_some` porque aquele avalia o argumento SEMPRE: com "tela
            // 0", o `n - 1` estoura o usize e derruba o programa em debug. Em
            // release o estouro dá a volta em silêncio e o teste passava.
            if n >= 1 && n <= monitores.len() {
                return Some(Escolha::Monitor(n - 1));
            }
            return None;
        }
        // "telafone", "monitoramento": não era um pedido de monitor, e sim
        // parte do nome de uma janela. Cai no caminho de baixo.
    }

    janelas
        .iter()
        .position(|j| j.to_lowercase().contains(&t))
        .map(Escolha::Janela)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::audio::Som;

    const MONITORES: [&str; 2] = ["Tela 1, 1920x1080", "Tela 2 (principal), 2560x1440"];

    fn monitores() -> Vec<String> {
        MONITORES.iter().map(|s| s.to_string()).collect()
    }
    fn janelas() -> Vec<String> {
        ["Elden Ring", "Discord", "Visual Studio Code"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn numero_de_tela_conta_a_partir_de_um() {
        assert_eq!(escolher("tela 1", &janelas(), &monitores()), Some(Escolha::Monitor(0)));
        assert_eq!(escolher("tela 2", &janelas(), &monitores()), Some(Escolha::Monitor(1)));
        assert_eq!(escolher("monitor 2", &janelas(), &monitores()), Some(Escolha::Monitor(1)));
        assert_eq!(escolher("tela2", &janelas(), &monitores()), Some(Escolha::Monitor(1)));
    }

    #[test]
    fn tela_sozinha_e_a_principal() {
        // É o que a pessoa quer dizer quando não especifica.
        assert_eq!(escolher("tela", &janelas(), &monitores()), Some(Escolha::Monitor(1)));
    }

    #[test]
    fn tela_que_nao_existe_nao_vira_a_primeira() {
        // Pedir a tela 5 e escolher a 1 é pior que recusar: a pessoa
        // transmitiria a tela errada sem saber.
        assert_eq!(escolher("tela 5", &janelas(), &monitores()), None);
        assert_eq!(escolher("tela 0", &janelas(), &monitores()), None);
    }

    #[test]
    fn janela_e_achada_por_pedaco_do_nome_sem_ligar_pra_caixa() {
        assert_eq!(escolher("elden", &janelas(), &monitores()), Some(Escolha::Janela(0)));
        assert_eq!(escolher("CODE", &janelas(), &monitores()), Some(Escolha::Janela(2)));
    }

    #[test]
    fn palavra_que_so_comeca_com_tela_continua_sendo_janela() {
        // "monitoramento" começa com "monitor" e não é um pedido de monitor.
        let js = vec!["Monitoramento da rede".to_string()];
        assert_eq!(escolher("monitoramento", &js, &monitores()), Some(Escolha::Janela(0)));
    }

    #[test]
    fn sem_monitor_nenhum_o_pedido_de_tela_nao_inventa() {
        assert_eq!(escolher("tela", &janelas(), &[]), None);
        assert_eq!(escolher("tela 1", &janelas(), &[]), None);
    }

    #[test]
    fn texto_vazio_e_nome_que_nao_existe_nao_escolhem_nada() {
        assert_eq!(escolher("", &janelas(), &monitores()), None);
        assert_eq!(escolher("   ", &janelas(), &monitores()), None);
        assert_eq!(escolher("photoshop", &janelas(), &monitores()), None);
    }

    #[test]
    fn a_tela_inteira_vai_sem_a_call() {
        assert_eq!(som_do_monitor(Some(4242)), Som::TudoMenos(4242));
    }

    #[test]
    fn sem_discord_a_tela_inteira_vai_muda() {
        // E não "com tudo": tudo inclui as vozes da call, que voltariam com
        // atraso para quem está nela.
        assert_eq!(som_do_monitor(None), Som::Nenhum);
    }

    #[test]
    fn o_indice_pula_os_monitores_que_vem_na_frente() {
        // Errar esta conta aponta para a fonte vizinha sem dar erro nenhum.
        assert_eq!(indice_na_lista(Escolha::Monitor(0), 3), 0);
        assert_eq!(indice_na_lista(Escolha::Monitor(2), 3), 2);
        assert_eq!(indice_na_lista(Escolha::Janela(0), 3), 3);
        assert_eq!(indice_na_lista(Escolha::Janela(4), 3), 7);
        // Máquina sem monitor legível: a lista é só de janelas.
        assert_eq!(indice_na_lista(Escolha::Janela(0), 0), 0);
    }
}
