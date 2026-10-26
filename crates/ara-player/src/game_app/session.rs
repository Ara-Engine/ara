//! Background world preparation keeps menus, connection and loading responsive.
use super::*;
use ara_play::{Role,NetStart};

pub(super) struct Prepared {pub world:crate::world::World<ara_runtime::Runtime>,pub build_ms:f64,pub role:Role,pub net:Option<NetStart>}

impl App {
    pub(super) fn begin(&mut self,role:Role,address:Option<String>)->Result<()> {
        let root=self.root.clone();
        let project=self.project;
        let (send,receive)=std::sync::mpsc::channel();
        std::thread::Builder::new().name("outpost-load".into()).spawn(move||{
            let result=(||{
                let ticket=address.as_deref().map(lobby::join).transpose()?;
                let content=if project{content::project(&root)?}else{content::exported(&root)?};
                let net=match role {
                    Role::Local=>None,
                    Role::Host|Role::Dedicated=>{let clients=if role==Role::Dedicated{4}else{3};Some(NetStart{players:clients+1,clients:vec![None;clients as usize],join:None})},
                    Role::Client=>{let ticket=ticket.context("a client requires a host address")?;Some(NetStart{players:ticket.players,clients:vec![],join:Some(ticket.join)})},
                };
                let (world,build_ms)=World::prepare_game(&content.scene,net.as_ref(),content.source)?;
                Ok(Prepared{world,build_ms,role,net})
            })();
            let _=send.send(result);
        })?;
        self.loading=Some(receive);
        self.screen(Screen::Loading)
    }

    pub(super) fn poll_loading(&mut self)->Result<()> {
        let Some(receiver)=&self.loading else{return Ok(());};
        let result=match receiver.try_recv(){Ok(result)=>result,Err(std::sync::mpsc::TryRecvError::Empty)=>return Ok(()),Err(e)=>Err(e.into())};
        self.loading=None;
        match result {
            Ok(prepared)=>{
                let mut world=prepared.world.map_runtime(|runtime|Play::new(runtime,prepared.role,prepared.net.as_ref()))?;
                world.start(true,prepared.build_ms)?;
                if prepared.role==Role::Host||prepared.role==Role::Dedicated {self.lobby=Some(lobby::Lobby::bind(self.port,if prepared.role==Role::Dedicated{4}else{3})?);}
                if self.window.is_some(){if let Ok(device)=ara_audio::device::SdlBackend::open(48_000){self.audio=Some((world.audio_presenter()?,device));}}
                let address=format!("Crew channel: 127.0.0.1:{}",self.port);
                if let Some(surface)=self.surface(){surface.label("lobby-address",&address)?;}
                self.world=Some(world);
                self.screen(Screen::Lobby)?;
            },
            Err(error)=>{
                self.screen(Screen::Main)?;
                if let Some(surface)=self.surface(){surface.label("connection-error",&format!("Connection failed: {error:#}"))?;}
                else{return Err(error);}
            },
        }
        Ok(())
    }
}
