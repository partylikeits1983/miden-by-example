use miden_client::{ClientError, Felt};
use miden_crypto::hash::rpo::Rpo256 as Hasher;
use miden_objects::Word;

async fn find_hash(input: Word, target: u64) -> Word {
    let mut i: u64 = 0;
    loop {
        let nonce = vec![Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(i)];
        let input = vec![
            input[0], input[1], input[2], input[3], nonce[0], nonce[1], nonce[2], nonce[3],
        ];
        let digest: Word = Hasher::hash_elements(&input).into();
        let val: u64 = digest[0].into();

        println!("iter: {:?} target: {:?}", i, val);
        if val < target {
            let word: Word = [nonce[0], nonce[1], nonce[2], nonce[3]];
            return word;
        }
        i += 1;
    }
}

#[tokio::main]
async fn main() -> Result<(), ClientError> {
    let input = Word::default();
    let target = 11529215046068400;

    let result = find_hash(input, target).await;
    println!("result: {:?}", result);

    let word_0 = vec![
        Felt::new(333),
        Felt::new(333),
        Felt::new(333),
        Felt::new(333),
    ];
    let word_1 = vec![Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(48)];

    let input = vec![
        word_0[0], word_0[1], word_0[2], word_0[3], word_1[0], word_1[1], word_1[2], word_1[3],
    ];

    let digest = Hasher::hash_elements(&input);

    println!("input: {:?}", input);
    println!("digest: {:?}", digest);

    Ok(())
}
