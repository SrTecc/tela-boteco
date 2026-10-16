//! O login: provar ao site quem está transmitindo.
//!
//! O app não tem navegador, então não guarda cookie. O caminho é o mesmo de
//! qualquer aplicativo de desktop que fala com um site: ele **ouve numa porta
//! da própria máquina**, abre o navegador no login, e a volta do OAuth entrega
//! a sessão ali.
//!
//! O que fica guardado aqui é **só a sessão assinada pelo site**, nunca segredo
//! nenhum. O segredo que assina vive na Vercel; o app não o tem e não precisa
//! dele, porque quem confere a assinatura é o servidor. Isso importa: o
//! executável é um arquivo que qualquer um abre e lê.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;

/// Onde a sessão fica entre uma execução e outra.
fn arquivo() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("tela-boteco").join("sessao.txt")
}

/// A sessão guardada, se ainda valer.
///
/// O prazo é lido do próprio corpo da sessão, que é JSON em base64. Não dá para
/// conferir a assinatura aqui (o segredo não é nosso), e nem é preciso: uma
/// sessão adulterada seria recusada pelo site. O que se evita olhando o prazo é
/// abrir a transmissão e só então descobrir que expirou.
pub fn guardada() -> Option<String> {
    let texto = std::fs::read_to_string(arquivo()).ok()?;
    let valor = texto.trim().to_string();
    if valor.is_empty() || vencida(&valor) {
        return None;
    }
    Some(valor)
}

fn vencida(sessao: &str) -> bool {
    let Some(corpo) = sessao.split('.').next() else { return true };
    let Ok(bytes) = base64_url(corpo) else { return true };
    let Ok(texto) = String::from_utf8(bytes) else { return true };
    // Sem biblioteca de JSON para ler um número: procurar a chave é suficiente,
    // e erra para o lado seguro (sem prazo legível, trata como vencida).
    let Some(pos) = texto.find("\"exp\":") else { return true };
    let resto = &texto[pos + 6..];
    let numero: String = resto.chars().take_while(|c| c.is_ascii_digit()).collect();
    match numero.parse::<u128>() {
        Ok(exp) => {
            let agora = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            // Uma hora de folga: sessão que vence no meio da transmissão é pior
            // que sessão que pede login antes de começar.
            exp < agora + 3_600_000
        }
        Err(_) => true,
    }
}

fn base64_url(texto: &str) -> Result<Vec<u8>, ()> {
    const TABELA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut valores = Vec::with_capacity(texto.len());
    for c in texto.bytes() {
        match TABELA.iter().position(|&t| t == c) {
            Some(v) => valores.push(v as u32),
            None => return Err(()),
        }
    }
    let mut saida = Vec::with_capacity(valores.len() * 3 / 4);
    for pedaco in valores.chunks(4) {
        let mut acumulado = 0u32;
        for (i, v) in pedaco.iter().enumerate() {
            acumulado |= v << (18 - 6 * i);
        }
        let quantos = pedaco.len() * 6 / 8;
        for i in 0..quantos {
            saida.push(((acumulado >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Ok(saida)
}

/// Faz o login: abre o navegador e espera a volta.
///
/// Devolve a sessão, já guardada para a próxima vez.
pub fn entrar(site: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Porta zero: o sistema escolhe uma livre. Fixar porta é convidar o
    // conflito com outro programa e, pior, deixar previsível quem escuta.
    let ouvinte = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let porta = ouvinte.local_addr()?.port();

    let url = format!("{}/api/login?app={porta}", site.trim_end_matches('/'));
    println!("abrindo o navegador para entrar com o Discord...");
    println!("se ele não abrir sozinho, cola isto: {url}");
    abrir_navegador(&url);

    // Uma conexão só: a do navegador voltando. Quem chegar antes com outra
    // coisa é ignorado, porque o que importa é a volta com a sessão.
    for fluxo in ouvinte.incoming() {
        let mut fluxo = fluxo?;
        let mut leitor = BufReader::new(&fluxo);
        let mut pedido = String::new();
        leitor.read_line(&mut pedido)?;

        let Some(sessao) = sessao_da_linha(&pedido) else {
            responder(&mut fluxo, "Essa volta veio sem sessão. Tenta de novo pelo app.");
            continue;
        };

        responder(
            &mut fluxo,
            "Pronto! Pode fechar esta aba e voltar para o Tela do Boteco.",
        );
        guardar(&sessao)?;
        return Ok(sessao);
    }
    Err("o navegador não voltou".into())
}

/// Tira a sessão da linha de pedido HTTP (`GET /?sessao=... HTTP/1.1`).
pub fn sessao_da_linha(linha: &str) -> Option<String> {
    let caminho = linha.split_whitespace().nth(1)?;
    let consulta = caminho.split_once('?')?.1;
    for par in consulta.split('&') {
        if let Some(valor) = par.strip_prefix("sessao=") {
            let decodificado = percent_decode(valor);
            if !decodificado.is_empty() {
                return Some(decodificado);
            }
        }
    }
    None
}

/// Desfaz o `%XX` da URL. Só o necessário: a sessão é base64url mais um ponto.
fn percent_decode(texto: &str) -> String {
    let bytes = texto.as_bytes();
    let mut saida = String::with_capacity(texto.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&texto[i + 1..i + 3], 16) {
                saida.push(v as char);
                i += 3;
                continue;
            }
        }
        saida.push(bytes[i] as char);
        i += 1;
    }
    saida
}

fn responder(fluxo: &mut std::net::TcpStream, recado: &str) {
    let corpo = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Tela do Boteco</title>\
         <body style=\"background:#15181b;color:#ece7dd;font:16px system-ui;\
         display:flex;align-items:center;justify-content:center;height:100vh\">\
         <p>{recado}</p>"
    );
    let resposta = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
        corpo.len()
    );
    let _ = fluxo.write_all(resposta.as_bytes());
    let _ = fluxo.flush();
}

fn guardar(sessao: &str) -> std::io::Result<()> {
    let caminho = arquivo();
    if let Some(pasta) = caminho.parent() {
        std::fs::create_dir_all(pasta)?;
    }
    std::fs::write(caminho, sessao)
}

fn abrir_navegador(url: &str) {
    // `start` é um comando do próprio shell, não um programa, daí o `cmd /C`. O
    // par de aspas vazias é o título da janela: sem ele, o shell trata o
    // endereço como título e não abre nada.
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
}

#[cfg(test)]
mod testes {
    use super::*;

    // A volta do navegador Ã© a Ãºnica coisa que o app aceita de fora nesta
    // etapa, entÃ£o Ã© a que precisa aguentar entrada torta.
    #[test]
    fn tira_a_sessao_da_linha_de_pedido() {
        let linha = "GET /?sessao=abc.def HTTP/1.1";
        assert_eq!(sessao_da_linha(linha).as_deref(), Some("abc.def"));
    }

    #[test]
    fn desfaz_o_escape_da_url() {
        // O ponto que separa corpo e assinatura pode vir escapado.
        let linha = "GET /?sessao=corpo%2Eassin HTTP/1.1";
        assert_eq!(sessao_da_linha(linha).as_deref(), Some("corpo.assin"));
    }

    #[test]
    fn linha_sem_sessao_nao_vira_sessao_vazia() {
        assert!(sessao_da_linha("GET / HTTP/1.1").is_none());
        assert!(sessao_da_linha("GET /?outra=coisa HTTP/1.1").is_none());
        assert!(sessao_da_linha("GET /?sessao= HTTP/1.1").is_none());
        assert!(sessao_da_linha("").is_none());
        assert!(sessao_da_linha("lixo").is_none());
    }

    #[test]
    fn acha_a_sessao_mesmo_com_outros_parametros() {
        let linha = "GET /?a=1&sessao=xyz&b=2 HTTP/1.1";
        assert_eq!(sessao_da_linha(linha).as_deref(), Some("xyz"));
    }

    // O prazo Ã© lido do corpo da sessÃ£o sem conferir assinatura: o segredo nÃ£o
    // Ã© nosso, e quem confere Ã© o site. O que isto evita Ã© abrir a transmissÃ£o
    // e sÃ³ entÃ£o descobrir que a sessÃ£o venceu.
    #[test]
    fn sessao_sem_prazo_legivel_conta_como_vencida() {
        assert!(vencida(""));
        assert!(vencida("lixo"));
        assert!(vencida("semponto"));
        assert!(vencida("naoebase64!!!.assinatura"));
    }

    #[test]
    fn prazo_no_passado_e_vencido_e_no_futuro_nao() {
        let agora = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let monta = |exp: u128| {
            let corpo = format!("{{\"id\":\"1\",\"exp\":{exp}}}");
            format!("{}.assinatura", codificar(corpo.as_bytes()))
        };
        assert!(vencida(&monta(agora - 1000)), "ontem Ã© vencida");
        assert!(!vencida(&monta(agora + 86_400_000)), "amanhÃ£ nÃ£o Ã©");
        // A folga de uma hora existe porque sessÃ£o que morre no meio da
        // transmissÃ£o Ã© pior que sessÃ£o que pede login antes de comeÃ§ar.
        assert!(vencida(&monta(agora + 60_000)), "um minuto nÃ£o basta");
    }

    /// SÃ³ para o teste: monta base64url, que Ã© o que o site produz.
    fn codificar(dados: &[u8]) -> String {
        const TABELA: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut saida = String::new();
        for pedaco in dados.chunks(3) {
            let mut acumulado = 0u32;
            for (i, b) in pedaco.iter().enumerate() {
                acumulado |= (*b as u32) << (16 - 8 * i);
            }
            // Arredonda PARA CIMA: 3 bytes viram 4 caracteres, e a conta
            // ingenua (8/6 + 1) dava 5, corrompendo tudo que viesse depois.
            let quantos = (pedaco.len() * 8 + 5) / 6;
            for i in 0..quantos {
                saida.push(TABELA[((acumulado >> (18 - 6 * i)) & 0x3f) as usize] as char);
            }
        }
        saida
    }
}

