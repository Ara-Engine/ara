//! Licensed short effects and an intensity-driven score, authored through typed commands.
use super::*;

pub(super) fn author(ctx: &Ctx) -> Result<()> {
    for (path, id) in [
        ("outpost_scifi/Audio/doorOpen_000.ogg", "as_dooropen"),
        ("outpost_scifi/Audio/computerNoise_000.ogg", "as_robotnoise"),
        ("outpost_scifi/Audio/forceField_001.ogg", "as_pickup"),
        ("outpost_interface/Audio/click_001.ogg", "as_uiclick"),
        ("outpost_impact/Audio/footstep_grass_000.ogg", "as_footstep"),
    ] {
        ctx.sidecar(
            &format!("assets/third_party/{path}"),
            id,
            json!({"usage":"shortEffect","codec":"pcm"}),
        )?;
    }
    let path = "audio/outpost.mixer";
    if !ctx.root.join(path).is_file() {
        ctx.new_document(path, "mixer", "mx_outpost")?;
    }
    ctx.apply(path,"Author master, music and effects categories",["master","music","sfx"].into_iter().map(|name|{
        ("mixer.bus.set",json!({"id":format!("b_{name}"),"bus":{"name":name,"output":if name=="master"{""}else{"master"}}}))
    }).collect())?;
    ctx.apply("audio/outpost.music","Layer tension under the dusk ambience",vec![
        ("music.segment.set",json!({"id":"sg_dusk","segment":{"name":"dusk","bars":64,"tempo":[{"bpm":60}],"stems":[
            {"name":"lonelyhood","clip":"@assets/third_party/oga_lonelyhood/Lonelyhood.ogg#as_lonelyhood","gainDb":-10,"looping":true},
            {"name":"security","clip":"@assets/third_party/oga_freezer/freezer.ogg#as_drone","gainDb":-16,"intensity":[0.1,0.8],"looping":true}
        ]}}))
    ])
}
