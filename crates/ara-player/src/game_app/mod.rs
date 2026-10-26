//! A native game shell over the same World/Play roles used by the editor.
mod content;
mod lobby;
mod settings;
mod session;
use crate::{Args, World, device, game_ui::{Surface,Screen,Router}, input::{Mapper,DeviceEvent}, net::Play};
use anyhow::{Context, Result};
use ara_input::{Injection,ActionValue};
use ara_platform::Event;
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::Presenter;
use settings::Settings;
use std::{path::PathBuf,time::{Instant,Duration}};

#[derive(clap::Args)]
pub(crate) struct Options {
    /// Run the project's native main menu and co-op game shell.
    #[arg(long)] pub game: bool,
    /// Source Ara project. Omit to play an exported pack beside the executable/--root.
    #[arg(long)] project: Option<PathBuf>,
    /// Start a renderer-free four-player authority.
    #[arg(long)] dedicated: bool,
    /// Direct-connect loopback lobby port.
    #[arg(long,default_value_t=41090)] port: u16,
    /// Connect immediately to this loopback host (otherwise use the main menu).
    #[arg(long)] connect: Option<String>,
}

pub(crate) fn run(args: Args) -> Result<()> {
    let settings = Settings::load()?;
    let root = args.game.project.clone().or(args.standalone.root.clone()).unwrap_or_else(|| {
        std::env::current_exe().unwrap_or_default().parent().map(PathBuf::from).unwrap_or_default()
    });
    let initial = if args.game.project.is_some() { None } else { Some(content::exported(&root)?) };
    let ui = match &initial { Some(content) => content.scene.ui.clone().context("this game has no native UI")?, None => content::ui(&root)? };
    let no_window = args.game.dedicated || args.standalone.headless;
    let (window,presenter) = if no_window {
        (None,Presenter::new(settings.size(),Vec::new(),&args.gpu_cache))
    } else {
        let options = ara_runtime_render::RenderOptions {ray_tracing:false};
        let (mut window,presenter) = if args.standalone.offscreen {
            device::Windowed::open_offscreen("Outpost — Dusk shift",settings.size(),&args.gpu_cache,settings.quality(),options)?
        } else { device::Windowed::open("Outpost — Dusk shift",settings.size(),&args.gpu_cache,settings.quality(),options)? };
        window.game_ui = Some(Surface::plain(&ui,settings.size())?);
        (Some(window),presenter)
    };
    let mut app = App {root,project:args.game.project.is_some(),settings,window,presenter,ui,router:Router::default(),
        mapper:Mapper::default(),screen:Screen::Main,settings_return:Screen::Main,world:None,loading:None,
        lobby:None,port:args.game.port,frame:device::warm_frame(),audio:None,quit:false};
    app.screen(Screen::Main)?;
    if args.game.dedicated { app.begin(ara_play::Role::Dedicated,None)?; }
    if let Some(address) = args.game.connect { app.begin(ara_play::Role::Client,Some(address))?; }
    let mut previous = Instant::now();
    let mut frames = 0;
    while !app.quit && args.standalone.frames.is_none_or(|limit| frames < limit) {
        let now = Instant::now();
        app.tick(now.duration_since(previous))?;
        previous = now;
        frames += 1;
        if app.window.is_none() { std::thread::sleep(Duration::from_millis(2)); }
    }
    if let Some(mut window) = app.window.take() { window.shutdown(&app.presenter)?; }
    Ok(())
}

struct App {
    root: PathBuf,
    project: bool,
    settings: Settings,
    window: Option<device::Windowed>,
    presenter: Presenter,
    ui: ara_play::GameUi,
    router: Router,
    mapper: Mapper,
    screen: Screen,
    settings_return: Screen,
    world: Option<World>,
    loading: Option<std::sync::mpsc::Receiver<Result<session::Prepared>>>,
    lobby: Option<lobby::Lobby>,
    port: u16,
    frame: RenderFrame,
    audio: Option<(crate::world::Audio,ara_audio::device::SdlBackend)>,
    quit: bool,
}

impl App {
    fn surface(&mut self) -> Option<&mut Surface> { self.window.as_mut()?.game_ui.as_mut() }
    fn screen(&mut self, screen: Screen) -> Result<()> {
        self.screen = screen;
        if let Some(surface) = self.surface() { surface.screen(screen)?; }
        self.mapper.apply(DeviceEvent::FocusLost);
        Ok(())
    }
    fn action(&mut self, name: &str) -> Result<()> {
        match name {
            "solo" => self.begin(ara_play::Role::Local,None)?,
            "host" => self.begin(ara_play::Role::Host,None)?,
            "join" => {
                let address = self.surface().and_then(|s| s.value("address")).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| format!("127.0.0.1:{}",self.port));
                self.begin(ara_play::Role::Client,Some(address))?;
            },
            "ready" => {
                if let Some(world) = &mut self.world {
                    world.runtime.input(0,vec![Injection {player:world.runtime.player(),action:"Interact".into(),value:ActionValue::Button(true),hold_ticks:1}])?;
                    if !world.runtime.networked() { world.runtime.resume()?; }
                }
                self.screen(Screen::Game)?;
            },
            "settings" => { self.settings_return = self.screen; self.screen(Screen::Settings)?; },
            "quality" => { self.settings.quality = (self.settings.quality+1)%4; self.presenter.set_quality(self.settings.quality()); },
            "resolution" => { self.settings.resolution=(self.settings.resolution+1)%3; if let Some(window)=&self.window {window.game_window().set_size(self.settings.size())?;} },
            "window-mode" => { self.settings.window_mode = (self.settings.window_mode+1)%2; if let Some(window)=&self.window {window.game_window().toggle_fullscreen()?;} },
            "apply" => { self.read_settings(); self.settings.save()?; self.screen(self.settings_return)?; },
            "resume" => { if let Some(world)=&mut self.world { if !world.runtime.networked() {world.runtime.resume()?;} } self.screen(Screen::Game)?; },
            "leave" => { self.world=None; self.audio=None; self.lobby=None; self.loading=None; self.screen(Screen::Main)?; },
            "quit" => self.quit=true,
            _ => self.read_settings(),
        }
        self.settings_labels()
    }
    fn read_settings(&mut self) {
        let Some(surface)=self.window.as_mut().and_then(|w|w.game_ui.as_mut()) else{return;};
        for (name,target) in [("master",&mut self.settings.master),("music",&mut self.settings.music),
            ("sfx",&mut self.settings.sfx),("sensitivity",&mut self.settings.sensitivity)] {
            if let Some(value)=surface.value(name).and_then(|v|v.as_f64()) {*target=value as f32;}
        }
        if let Some(value)=surface.value("invert").and_then(|v|v.as_bool()) {self.settings.invert=value;}
    }
    fn settings_labels(&mut self) -> Result<()> {
        let quality=format!("Graphics: {}",Settings::QUALITIES[self.settings.quality%4]);
        let [w,h]=self.settings.size();
        let resolution=format!("Resolution: {w} × {h}");
        if let Some(surface)=self.surface() {surface.label("quality",&quality)?;surface.label("resolution",&resolution)?;}
        Ok(())
    }
    fn events(&mut self) -> Result<()> {
        let Some(window)=&mut self.window else{return Ok(());};
        let events=window.raw_events()?;
        for event in events {
            if matches!(event,Event::Quit) {self.quit=true;}
            if matches!(event,Event::Key{scancode:41,pressed:true,repeat:false}) {
                match self.screen {
                    Screen::Game=>{if let Some(world)=&mut self.world {if !world.runtime.networked(){world.runtime.pause()?;}} self.screen(Screen::Pause)?;},
                    Screen::Pause=>self.action("resume")?, Screen::Settings=>self.screen(self.settings_return)?,
                    _=>{},
                }
            }
            if self.screen==Screen::Game {
                match event {
                    Event::Key{scancode,pressed,repeat:false}=>self.mapper.apply(DeviceEvent::Key{scancode,down:pressed}),
                    Event::MouseButton{button,pressed}=>self.mapper.apply(DeviceEvent::MouseButton{button,down:pressed}),
                    Event::MouseMotion{delta,..}=>self.mapper.apply(DeviceEvent::MouseMotion{dx:delta[0]*self.settings.sensitivity,dy:delta[1]*self.settings.sensitivity*if self.settings.invert{-1.}else{1.}}),
                    _=>{},
                }
            } else if let Some(surface)=self.window.as_mut().and_then(|w|w.game_ui.as_mut()) {self.router.event(surface,&event)?;}
        }
        let actions = self.window.as_mut().and_then(|w|w.game_ui.as_mut()).map(|s|self.router.actions(s)).unwrap_or_default();
        for action in actions {self.action(&action.name)?;}
        Ok(())
    }
    fn tick(&mut self, elapsed: Duration) -> Result<()> {
        self.events()?;
        self.poll_loading()?;
        if let Some(world)=&mut self.world {
            if let (Some(lobby),Play::Host(host))=(&mut self.lobby,&mut world.runtime) {lobby.poll(host)?;}
            let inputs=self.mapper.injections(world.runtime.player());
            if !inputs.is_empty(){world.runtime.input(0,inputs)?;}
            world.runtime.advance(elapsed)?;
            world.mark();
            if self.window.is_some(){world.extract_frame(&mut self.frame);}
            if let Some(window)=&mut self.window {
                if let Some(surface)=&mut window.game_ui {surface.update(&world.runtime,world.runtime.player().0,window.window_size()?)?;surface.screen(self.screen)?;}
            }
        }
        if let Some(window)=&mut self.window {
            if let Some(world)=&self.world {window.present(&mut self.presenter,&self.frame,||world.fidelity_content(&self.frame))?;}
            else {window.present(&mut self.presenter,&self.frame,||Ok(Default::default()))?;}
        }
        if let (Some(world),Some((audio,device)))=(&mut self.world,&mut self.audio) {
            let inputs=world.audio_inputs()?;
            crate::world::update_audio(audio,world.runtime.world(),&crate::world::listener(&self.frame),&inputs)?;
            audio.volumes(self.settings.master,self.settings.music,self.settings.sfx)?;
            audio.pump(device,crate::AUDIO_WATERMARK)?;
        }
        Ok(())
    }
}
