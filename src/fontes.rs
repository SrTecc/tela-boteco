//! Escolher o que transmitir: uma janela ou um monitor.
//!
//! A regra mora aqui, pura e sobre ROTULOS, e nao sobre handles do Windows,
//! por dois motivos. Da para testa-la sem monitor nenhum e sem janela nenhuma,
//! numa maquina de teste que nem tem tela. E a interface grafica, quando
//! existir, vai precisar exatamente desta regra: a lista que ela mostra e a
//! mesma lista que a linha de comando resolve.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escolha {
    Janela(usize),
    Monitor(usize),
}

/// Quem o texto escolhe, entre as janelas e os monitores.
///
/// "tela", "tela 2", "monitor 2" e "tela2" apontam para monitor; qualquer
/// outra coisa e procurada como pedaco do nome de uma janela. Sozinho,
/// "tela" vale o monitor principal, porque e o que a pessoa quer dizer quando
/// nao especifica.
///
/// Os rotulos de monitor sao os do `monitores::rotulo_de`, e e deles que sai
/// qual e o principal.
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
            // Sem numero: o principal, e na falta dele o primeiro da lista.
            let principal = monitores
                .iter()
                .position(|r| r.to_lowercase().contains("principal"))
                .unwrap_or(0);
            return (!monitores.is_empty()).then_some(Escolha::Monitor(principal));
        }
        if let Ok(n) = resto.parse::<usize>() {
            // Conta a partir de 1, como o rotulo mostra.
            return (n >= 1 && n <= monitores.len()).then_some(Escolha::Monitor(n - 1));
        }
        // "telafone", "monitoramento": nao era um pedido de monitor, e sim
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
        // E o que a pessoa quer dizer quando nao especifica.
        assert_eq!(escolher("tela", &janelas(), &monitores()), Some(Escolha::Monitor(1)));
    }

    #[test]
    fn tela_que_nao_existe_nao_vira_a_primeira() {
        // Pedir a tela 5 e escolher a 1 e pior que recusar: a pessoa
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
        // "monitoramento" comeca com "monitor" e nao e um pedido de monitor.
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
}
