//! Tela do Boteco: o app de transmissão.
//!
//! Primeiro marco da Fase 1: listar as janelas transmissíveis. Sem interface e
//! sem rede ainda, de propósito. O plano é atacar o risco técnico antes do
//! acabamento, e a ordem dos riscos é captura, codificação e só então rede.

mod janelas;

fn main() {
    let lista = janelas::listar();

    println!("{} janela(s) que dariam para transmitir:\n", lista.len());
    println!(
        "{:<10} {:<22} {:>11}   {}",
        "pid", "executável", "tamanho", "título"
    );
    println!("{}", "-".repeat(94));

    for j in &lista {
        let titulo: String = j.titulo.chars().take(44).collect();
        let tamanho = if j.minimizada {
            "minimizada".to_string()
        } else {
            format!("{}x{}", j.largura, j.altura)
        };
        println!(
            "{:<10} {:<22} {:>11}   {}",
            j.pid, j.executavel, tamanho, titulo
        );
    }

    if lista.is_empty() {
        println!("(nenhuma: isso é suspeito, e vale olhar os filtros em janelas.rs)");
    }
}
