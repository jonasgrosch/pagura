#![forbid(unsafe_code)]
//! Qualification-only memory lifetime probe through the safe public facade.
use std::{error::Error, fs, io::{Read, Write}, sync::Arc};

const REVISION: &str=env!("PAGURA_QUAL_SOURCE_REVISION");

fn checkpoint(phase:&str, iteration:usize, retained:Option<usize>) -> Result<(),Box<dyn Error>> {
    let retained=retained.map_or_else(|| "null".to_owned(),|n|n.to_string());
    println!("{{\"source_revision\":\"{REVISION}\",\"phase\":\"{phase}\",\"iteration\":{iteration},\"store_retained_bytes\":{retained}}}");
    std::io::stdout().flush()?;
    std::io::stdin().read_exact(&mut [0])?;
    Ok(())
}

fn main()->Result<(),Box<dyn Error>> {
    let args:Vec<_>=std::env::args().collect();
    if args.len()!=7 {return Err("input output dpi capacity samples rgb|rgba".into());}
    let dpi:f64=args[3].parse()?;
    let capacity:usize=args[4].parse()?;
    let count:usize=args[5].parse()?;
    if count>1000 {return Err("probe iteration cap".into());}
    let rgb=match args[6].as_str(){"rgb"=>true,"rgba"=>false,_=>return Err("rgb|rgba".into())};
    let document=Arc::new(pagura::Document::open(fs::read(&args[1])?,&pagura::OpenOptions {recovery:pagura::Recovery::Repair,..pagura::OpenOptions::default()})?);
    let provider=pagura::provider::default_provider(document.limits());
    let context=pagura::RenderContext::with_store_capacity(document,provider,capacity);
    let options=pagura::RenderOptions { background:if rgb {pagura::Rgba8::WHITE} else {pagura::Rgba8::TRANSPARENT}, ..pagura::RenderOptions::from_dpi(dpi) };
    let cold=context.render_page(context.document().page(0)?,&options)?;
    let width=cold.width();let height=cold.height();
    let mut expected_rgba=None;
    let mut expected_rgb=None;
    let initial_rgb=cold.to_rgb_over(pagura::Rgba8::WHITE);
    let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&args[2])?;
    write!(file,"P6\n{width} {height}\n255\n")?;file.write_all(&initial_rgb)?;drop(file);
    fs::OpenOptions::new().write(true).create_new(true).open(format!("{}.rgba",args[2]))?.write_all(cold.data())?;
    if rgb {expected_rgb=Some(initial_rgb);drop(cold);} else {expected_rgba=Some(cold);drop(initial_rgb);}
    checkpoint("render-dropped",0,Some(context.retained_bytes()))?;
    for iteration in 1..=count {
        let pixels=context.render_page(context.document().page(0)?,&options)?;
        if (pixels.width(),pixels.height())!=(width,height){return Err("geometry changed".into());}
        if let Some(expected)=&expected_rgb {
            if &pixels.to_rgb_over(pagura::Rgba8::WHITE)!=expected {return Err("RGB pixels changed".into());}
        } else if let Some(expected)=&expected_rgba {
            if pixels.data()!=expected.data(){return Err("RGBA pixels changed".into());}
        }
        drop(pixels);
        checkpoint("render-dropped",iteration,Some(context.retained_bytes()))?;
    }
    drop(expected_rgba);drop(expected_rgb);drop(context);
    checkpoint("context-dropped",count+1,None)?;
    Ok(())
}
