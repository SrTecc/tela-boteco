//! O selo da casa: um copo de chope, desenhado.
//!
//! Desenhado e não arquivo de imagem, pelo mesmo motivo dos ornamentos dos
//! cards do bot: não depende de asset nenhum, sai no tamanho que precisar e
//! acompanha a paleta. Serve ao ícone da janela e ao da bandeja, que o Windows
//! pede em tamanhos diferentes.
//!
//! O desenho é grosso de propósito. Na bandeja ele aparece com 16 pixels de
//! lado, e ali alça, vinco e contorno viram sujeira: o que resta legível é a
//! silhueta, que é um bloco âmbar com uma faixa clara em cima.

/// A espuma, o âmbar do chope e o fundo do copo, do tema da casa.
const ESPUMA: (f32, f32, f32) = (236.0, 231.0, 221.0);
const AMBAR: (f32, f32, f32) = (240.0, 166.0, 44.0);
const AMBAR_FUNDO: (f32, f32, f32) = (176.0, 118.0, 26.0);

/// Quantas amostras por lado de pixel. Sem isto a borda vira escada, e escada
/// de 16 pixels é o que separa um ícone de um borrão.
const AMOSTRAS: u32 = 4;

/// O ícone em RGBA, com transparência, do tamanho pedido.
pub fn icone(lado: u32) -> Vec<u8> {
    let mut saida = vec![0u8; (lado * lado * 4) as usize];
    let por_pixel = (AMOSTRAS * AMOSTRAS) as f32;

    for y in 0..lado {
        for x in 0..lado {
            let (mut r, mut g, mut b, mut cobertas) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..AMOSTRAS {
                for sx in 0..AMOSTRAS {
                    let px = (x as f32 + (sx as f32 + 0.5) / AMOSTRAS as f32) / lado as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / AMOSTRAS as f32) / lado as f32;
                    if let Some((cr, cg, cb)) = cor_em(px, py) {
                        r += cr;
                        g += cg;
                        b += cb;
                        cobertas += 1.0;
                    }
                }
            }
            if cobertas > 0.0 {
                let i = ((y * lado + x) * 4) as usize;
                // Cor é a média das amostras que pintaram; a transparência é a
                // fração de amostras cobertas. Alfa direto, não multiplicado:
                // é o que o Windows espera receber.
                saida[i] = (r / cobertas) as u8;
                saida[i + 1] = (g / cobertas) as u8;
                saida[i + 2] = (b / cobertas) as u8;
                saida[i + 3] = (cobertas / por_pixel * 255.0) as u8;
            }
        }
    }
    saida
}

/// A cor num ponto do desenho, em coordenadas de 0 a 1, ou nada onde é vazio.
fn cor_em(x: f32, y: f32) -> Option<(f32, f32, f32)> {
    // A espuma vem primeiro: ela transborda e cobre a boca do copo.
    if dentro(x, y, 0.17, 0.08, 0.83, 0.33, 0.11) {
        return Some(ESPUMA);
    }
    if dentro(x, y, 0.24, 0.26, 0.76, 0.93, 0.08) {
        // O chope escurece para o fundo. Num ícone de 16 pixels isso quase não
        // aparece, e no de 64 é o que o faz parecer um copo e não um retângulo.
        let fundo = ((y - 0.26) / (0.93 - 0.26)).clamp(0.0, 1.0);
        return Some(misturar(AMBAR, AMBAR_FUNDO, fundo));
    }
    None
}

/// Retângulo de cantos arredondados, pelo truque do ponto mais próximo: a
/// distância até o miolo do retângulo encolhido diz tudo, e vale para os
/// quatro cantos de uma vez.
fn dentro(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, raio: f32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let dx = (x0 + raio - x).max(x - (x1 - raio)).max(0.0);
    let dy = (y0 + raio - y).max(y - (y1 - raio)).max(0.0);
    dx * dx + dy * dy <= raio * raio
}

fn misturar(a: (f32, f32, f32), b: (f32, f32, f32), quanto: f32) -> (f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * quanto,
        a.1 + (b.1 - a.1) * quanto,
        a.2 + (b.2 - a.2) * quanto,
    )
}

#[cfg(test)]
mod testes {
    use super::*;

    fn pixel(dados: &[u8], lado: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * lado + x) * 4) as usize;
        (dados[i], dados[i + 1], dados[i + 2], dados[i + 3])
    }

    #[test]
    fn o_tamanho_e_o_pedido() {
        for lado in [16u32, 32, 64] {
            assert_eq!(icone(lado).len(), (lado * lado * 4) as usize);
        }
    }

    #[test]
    fn os_cantos_sao_vazios() {
        // Ícone que preenche o quadrado inteiro vira um borrão colorido na
        // bandeja, encostado nos vizinhos.
        let lado = 32;
        let d = icone(lado);
        for (x, y) in [(0, 0), (lado - 1, 0), (0, lado - 1), (lado - 1, lado - 1)] {
            assert_eq!(pixel(&d, lado, x, y).3, 0, "o canto {x},{y} devia ser vazio");
        }
    }

    #[test]
    fn o_meio_e_chope_e_o_alto_e_espuma() {
        let lado = 32;
        let d = icone(lado);
        let (r, g, b, a) = pixel(&d, lado, lado / 2, lado / 2);
        assert_eq!(a, 255, "o meio do copo tem de ser opaco");
        assert!(r > 150 && g > 90 && b < 90, "o meio devia ser âmbar, veio {r},{g},{b}");

        let (r, g, b, a) = pixel(&d, lado, lado / 2, lado / 5);
        assert_eq!(a, 255);
        assert!(r > 200 && g > 200 && b > 190, "o alto devia ser espuma, veio {r},{g},{b}");
    }

    #[test]
    fn mesmo_com_dezesseis_pixels_o_copo_aparece() {
        // É o tamanho em que ele sai na bandeja, e o desenho foi feito para
        // sobreviver a ele: se o miolo sumir aqui, sumiu de verdade.
        let lado = 16;
        let d = icone(lado);
        let opacos = (0..lado * lado)
            .filter(|i| d[(i * 4 + 3) as usize] > 128)
            .count();
        assert!(opacos > 60, "quase nada ficou visível: {opacos} pixels");
        assert!(opacos < 200, "encheu o quadrado inteiro: {opacos} pixels");
    }
}
