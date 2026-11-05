//! O tema de boteco, o mesmo do site e dos cards do bot.
//!
//! A paleta e a tipografia não são escolha deste arquivo: são as da casa, e
//! estão repetidas aqui porque o app não compartilha código com o site. O
//! balcão, a espuma e o âmbar do chope saem do `cardsLimpo.js`; a slab de
//! rótulo de cerveja e a condensada de letreiro saem do `cards.js`.
//!
//! Duas regras herdadas de lá, que custaram rejeição de paleta antes:
//!
//! - **Um acento só.** A diferença entre um bloco e outro é o ícone ou o peso,
//!   nunca uma cor nova. Meia dúzia de matizes saturados sem relação entre si é
//!   o que dá cara de template de máquina.
//! - **A brasa é só status.** Ao vivo e erro. Ali a cor carrega significado, e
//!   trocar por âmbar apagaria o aviso.
//!
//! O fundo é levemente FRIO de propósito: é o que faz o âmbar saltar. Em fundo
//! quente, âmbar sobre marrom vira a mesma cor.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Stroke, TextStyle};
use std::sync::Arc;

pub const BALCAO: Color32 = Color32::from_rgb(0x15, 0x18, 0x1b);
pub const PAINEL: Color32 = Color32::from_rgb(0x1c, 0x20, 0x24);
pub const PAINEL_ALTO: Color32 = Color32::from_rgb(0x26, 0x2b, 0x30);
pub const FUNDO_FUNDO: Color32 = Color32::from_rgb(0x10, 0x12, 0x14);
pub const TINTA: Color32 = Color32::from_rgb(0xec, 0xe7, 0xdd);
pub const TINTA_FRACA: Color32 = Color32::from_rgb(0x9a, 0xa1, 0xa8);
pub const AMBAR: Color32 = Color32::from_rgb(0xf0, 0xa6, 0x2c);
pub const AMBAR_FOSCO: Color32 = Color32::from_rgb(0xb9, 0x80, 0x1f);
pub const AMBAR_CLARO: Color32 = Color32::from_rgb(0xff, 0xc7, 0x66);
/// Só status: ao vivo e erro.
pub const BRASA: Color32 = Color32::from_rgb(0xe0, 0x62, 0x2a);
/// A linha fina entre blocos. No site e espuma a 13% sobre o painel; aqui e a
/// mesma cor ja misturada, porque constante nao mistura em tempo de compilacao.
pub const LINHA: Color32 = Color32::from_rgb(0x37, 0x3a, 0x3c);

/// A família do título. Nome próprio porque é uma segunda família, e não um
/// tamanho diferente da mesma.
const TITULO: &str = "titulo";

/// As fontes vão EMBARCADAS no executável, como no bot.
///
/// Dependeria da máquina de quem baixa, senão: "Alfa Slab One" não está
/// instalada em PC nenhum, e o que sairia é a fonte de reserva, que é
/// exatamente a cara genérica que o tema existe para não ter.
const ALFA_SLAB: &[u8] = include_bytes!("../assets/fontes/AlfaSlabOne-Regular.ttf");
const OSWALD: &[u8] = include_bytes!("../assets/fontes/Oswald-Variable.ttf");

pub fn aplicar(ctx: &egui::Context) {
    let mut fontes = FontDefinitions::default();
    fontes
        .font_data
        .insert("oswald".into(), Arc::new(FontData::from_static(OSWALD)));
    fontes
        .font_data
        .insert(TITULO.into(), Arc::new(FontData::from_static(ALFA_SLAB)));

    // A Oswald entra NA FRENTE das de reserva, e não no lugar delas: o que ela
    // não tiver (um emoji no nome de uma janela, por exemplo) cai para a de
    // trás em vez de virar quadradinho.
    fontes
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "oswald".into());
    fontes
        .families
        .insert(FontFamily::Name(TITULO.into()), vec![TITULO.into()]);
    ctx.set_fonts(fontes);

    // O app e escuro sempre: o tema da casa e balcao de boteco, e seguir o
    // tema claro do Windows deixaria a espuma branca sobre fundo branco.
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut estilo = (*ctx.global_style()).clone();

    estilo.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Name(TITULO.into()))),
        (TextStyle::Body, FontId::new(16.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(17.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut estilo.visuals;
    v.dark_mode = true;
    v.panel_fill = BALCAO;
    v.window_fill = PAINEL;
    v.extreme_bg_color = FUNDO_FUNDO;
    v.window_stroke = Stroke::new(1.0, LINHA);
    v.hyperlink_color = AMBAR;

    // Seleção em âmbar translúcido: a linha escolhida da lista precisa ficar
    // óbvia sem virar um bloco de cor que apaga o texto em cima dela.
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xf0, 0xa6, 0x2c, 46);
    v.selection.stroke = Stroke::new(1.0, TINTA);

    for (parte, fundo, borda, letra) in [
        (&mut v.widgets.noninteractive, PAINEL, LINHA, TINTA),
        (&mut v.widgets.inactive, PAINEL_ALTO, LINHA, TINTA),
        (&mut v.widgets.hovered, Color32::from_rgb(0x2f, 0x35, 0x3b), AMBAR_FOSCO, AMBAR_CLARO),
        (&mut v.widgets.active, AMBAR_FOSCO, AMBAR, BALCAO),
        (&mut v.widgets.open, PAINEL_ALTO, AMBAR_FOSCO, TINTA),
    ] {
        parte.bg_fill = fundo;
        parte.weak_bg_fill = fundo;
        parte.bg_stroke = Stroke::new(1.0, borda);
        parte.fg_stroke = Stroke::new(1.0, letra);
        parte.corner_radius = CornerRadius::same(6);
        parte.expansion = 0.0;
    }

    estilo.spacing.item_spacing = egui::vec2(8.0, 8.0);
    estilo.spacing.button_padding = egui::vec2(12.0, 7.0);
    estilo.spacing.interact_size.y = 30.0;
    estilo.spacing.scroll.bar_width = 8.0;

    // Nos dois temas, para nao depender do que o sistema diz.
    ctx.all_styles_mut(move |s| *s = estilo.clone());
}

/// A moldura de um bloco: o painel sobre o balcão.
pub fn bloco() -> egui::Frame {
    egui::Frame::new()
        .fill(PAINEL)
        .stroke(Stroke::new(1.0, LINHA))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(12)
}

/// Texto miúdo e apagado: a segunda linha, o detalhe, a explicação.
pub fn miudo(texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto).small().color(TINTA_FRACA)
}

/// Rótulo de seção: caixa alta é trabalho da condensada, nunca da slab, que
/// vira mancha em caixa alta.
pub fn rotulo(texto: &str) -> egui::RichText {
    egui::RichText::new(texto.to_uppercase())
        .small()
        .color(AMBAR_FOSCO)
}
