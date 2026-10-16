//! Os presets de qualidade.
//!
//! O pedido do dono foi "qualidade muito boa de imagem, com a opção de definir
//! as qualidades, entendendo que mais qualidade é mais processamento". Então o
//! que um preset mexe são as três coisas que custam, nesta ordem de peso:
//!
//! 1. **A resolução**, que é a que mais pesa, e sai de graça: o conversor de
//!    BGRA para NV12 já roda no chip da placa, e o VideoProcessor escala no
//!    mesmo passo. Mandar 720p em vez de 1080p não custa um passo a mais, custa
//!    menos trabalho no que já existia.
//! 2. **O bitrate**, que não pesa na CPU mas pesa no upload, e o upload é o
//!    número que manda aqui: quem transmite envia o vídeo uma vez por
//!    espectador.
//! 3. **O fps**, que é o menos interessante de cortar. A captura do Windows só
//!    entrega quadro quando a imagem muda, então uma tela parada já não gasta
//!    nada, e derrubar o fps estraga justamente o caso que importa, que é jogo.
//!
//! Por isso nenhum preset desce de 30: abaixo disso não parece economia, parece
//! defeito.

/// Um preset. `altura_max` 0 quer dizer "não mexe no tamanho".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Qualidade {
    pub id: &'static str,
    pub rotulo: &'static str,
    pub altura_max: u32,
    pub fps: u32,
    pub bitrate: u32,
}

pub const PRESETS: [Qualidade; 4] = [
    Qualidade {
        id: "leve",
        rotulo: "Leve — 720p 30fps, pra não atrapalhar o jogo",
        altura_max: 720,
        fps: 30,
        bitrate: 2_500_000,
    },
    Qualidade {
        id: "equilibrado",
        rotulo: "Equilibrado — 1080p 30fps",
        altura_max: 1080,
        fps: 30,
        bitrate: 5_000_000,
    },
    Qualidade {
        id: "nitido",
        rotulo: "Nítido — 1080p 60fps, bom pra jogo",
        altura_max: 1080,
        fps: 60,
        bitrate: 8_000_000,
    },
    Qualidade {
        id: "maximo",
        rotulo: "Máximo — tamanho original 60fps, exige upload",
        altura_max: 0,
        fps: 60,
        bitrate: 12_000_000,
    },
];

/// O padrão, quando ninguém escolheu.
pub fn padrao() -> Qualidade {
    PRESETS[2]
}

pub fn por_id(id: &str) -> Option<Qualidade> {
    let id = id.trim().to_lowercase();
    PRESETS.iter().copied().find(|q| q.id == id)
}

/// O tamanho que vai ao codificador, dado o tamanho da janela.
///
/// Três regras, e a segunda é a que não pode ser esquecida:
///
/// - **Só diminui.** Janela de 720p num preset de 1080p vai como 720p: subir
///   resolução não cria detalhe nenhum, só gasta upload inventando pixel.
/// - **Dimensão PAR**, nas duas. O H.264 amostra a cor de dois em dois pixels,
///   e largura ímpar faz o codificador recusar o formato. Já aconteceu aqui,
///   com uma janela de 1297 de largura.
/// - **Proporção mantida**, senão a imagem chega esticada.
pub fn dimensoes(largura: u32, altura: u32, altura_max: u32) -> (u32, u32) {
    let par = |n: u32| n.max(2) & !1;
    if altura_max == 0 || altura <= altura_max || altura == 0 {
        return (par(largura), par(altura));
    }
    let escala = altura_max as f64 / altura as f64;
    (par((largura as f64 * escala).round() as u32), par(altura_max))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn nao_aumenta_janela_pequena() {
        // 720p num preset de 1080p continua 720p: subir nao cria detalhe.
        assert_eq!(dimensoes(1280, 720, 1080), (1280, 720));
        assert_eq!(dimensoes(800, 600, 1080), (800, 600));
    }

    #[test]
    fn diminui_mantendo_a_proporcao() {
        assert_eq!(dimensoes(2560, 1440, 1080), (1920, 1080));
        assert_eq!(dimensoes(3840, 2160, 720), (1280, 720));
    }

    #[test]
    fn a_largura_sai_sempre_par() {
        // A janela de 1297 de largura é real: o codificador recusou o formato.
        assert_eq!(dimensoes(1297, 1000, 0).0 % 2, 0);
        assert_eq!(dimensoes(1297, 1000, 1080).0 % 2, 0);
        // 1366x768 escalado para 720 dá 1281,25: tem de cair para par.
        let (l, a) = dimensoes(1366, 768, 720);
        assert_eq!(l % 2, 0);
        assert_eq!(a, 720);
    }

    #[test]
    fn altura_max_zero_nao_mexe_no_tamanho() {
        assert_eq!(dimensoes(3840, 2160, 0), (3840, 2160));
    }

    #[test]
    fn janela_minuscula_nao_vira_zero() {
        // Zero quebraria a criacao da textura, e uma janela de 1px existe.
        assert_eq!(dimensoes(1, 1, 720), (2, 2));
    }

    #[test]
    fn nenhum_preset_desce_de_30fps() {
        for q in PRESETS {
            assert!(q.fps >= 30, "{} tem {} fps", q.id, q.fps);
            assert!(q.bitrate >= 1_000_000, "{}", q.id);
        }
    }

    #[test]
    fn os_ids_sao_unicos_e_o_padrao_existe() {
        let mut ids: Vec<_> = PRESETS.iter().map(|q| q.id).collect();
        ids.sort_unstable();
        let antes = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), antes);
        assert!(por_id(padrao().id).is_some());
        assert!(por_id("NITIDO").is_some(), "o id nao distingue maiuscula");
        assert!(por_id("inventado").is_none());
    }
}
