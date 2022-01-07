use std::path::Path;

use vidseq::VideoSequence;

// This saves a frame from every 1000 frames
fn main() -> anyhow::Result<()> {
    let mut seq = VideoSequence::open(Path::new("./video.mp4"))?;

    println!("original seq is {} long", seq.len());

    for i in 0..seq.len() {
        if i % 1000 != 0 {
            continue;
        }

        save_image(&mut seq, i)?;
    }

    Ok(())
}

fn save_image(seq: &mut VideoSequence, index: u64) -> anyhow::Result<()> {
    let img = seq.get_frame(index)?;

    if let Some(img) = img {
        img.save(Path::new(&format!("frames/{}.jpeg", index)))?;

        println!("written image {}", index);

        return Ok(());
    } else {
        return Err(anyhow::anyhow!("got empty image on {}", index));
    }
}
