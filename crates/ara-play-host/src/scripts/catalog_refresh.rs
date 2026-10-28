//! One nonblocking authority mailbox per world; only the latest reload publishes.
use crate::{PlayService,World,codes,fail};
use ara_diag::Diagnostic;
use ara_host::Cx;
use ara_play::method;
use serde_json::{Value,json};
use std::{collections::BTreeMap,time::{Duration,Instant}};

const TIMEOUT:Duration=Duration::from_secs(120);
struct Pending {request:u64,generation:u64,began:Instant}
#[derive(Default)]
struct State {generation:u64,published:u64,error:Option<String>}
#[derive(Default)]
pub(crate) struct CatalogRefresh {
    next:u64,
    pending:BTreeMap<String,Pending>,
    states:BTreeMap<String,State>,
}
impl CatalogRefresh {
    fn request(&mut self,id:&str,world:&mut World)->Result<(),Vec<Diagnostic>> {
        self.next+=1;
        let state=self.states.entry(id.into()).or_default();
        state.generation=self.next; state.error=None;
        if self.pending.contains_key(id) {return Ok(())}
        let request=match request(world) {Ok(request)=>request,Err(problems)=>{
            state.error=Some(format!("{problems:?}"));return Err(problems)
        }};
        self.pending.insert(id.into(),Pending{request,generation:state.generation,began:Instant::now()});
        Ok(())
    }
    pub(crate) fn invalidate(&mut self,id:&str,world:&mut World) {
        if let Some(pending)=self.pending.remove(id)
            && let Some((_,player))=world.players.first_mut()
        {player.forget_request(pending.request)}
        self.states.remove(id);
    }
    pub(crate) fn ready(&mut self,id:&str,result:&Result<Value,Vec<Diagnostic>>) {
        self.next+=1;
        let state=self.states.entry(id.into()).or_default();
        state.generation=self.next;
        match result {
            Ok(_)=>{state.published=state.generation;state.error=None;}
            Err(problems)=>state.error=Some(format!("{problems:?}")),
        }
    }
    pub(crate) fn refreshing(&self,id:&str)->bool {self.pending.contains_key(id)}
    pub(crate) fn stale(&self,id:&str)->bool {
        self.states.get(id).is_some_and(|state|state.generation!=state.published || state.error.is_some())
    }
    pub(crate) fn status(&self,id:&str)->Value {
        self.states.get(id).map_or(Value::Null,|state|json!({"generation":state.generation,
            "publishedGeneration":state.published,"error":state.error}))
    }
    fn poll(&mut self,worlds:&mut BTreeMap<String,World>,cx:&mut Cx<'_>) {
        let states=&mut self.states;
        self.pending.retain(|id,pending| {
            let Some(world)=worlds.get_mut(id) else {return false};
            let state=states.get_mut(id).expect("a pending catalog owns its generation");
            let Some((_,player))=world.players.first_mut() else {
                state.error=Some("C# command authority was removed".into());
                cx.log(Diagnostic::error(codes::PLAY,"C# command authority was removed"));return false
            };
            if world.fault.is_some() || pending.began.elapsed()>=TIMEOUT {
                player.forget_request(pending.request);
                let message=if world.fault.is_some(){"C# command refresh lost its authority"}else{"C# command refresh did not answer within 120 seconds"};
                state.error=Some(message.into());cx.log(Diagnostic::error(codes::PLAY,message));return false
            }
            let Some(result)=player.response(pending.request) else {return true};
            if pending.generation!=state.generation {
                // A Reload landed while this RPC was in flight. Its old result
                // never replaces the last-good catalog; request the newest once.
                if let Err(error)=&result {cx.log(Diagnostic::warning(codes::PLAY,format!("Discarded obsolete C# command refresh error: {error}")))}
                match request(world) {
                    Ok(request)=>{pending.request=request;pending.generation=state.generation;pending.began=Instant::now();return true}
                    Err(problems)=>{state.error=Some(format!("{problems:?}"));for problem in problems {cx.log(problem)}return false}
                }
            }
            match result {
                Ok(catalog) if catalog.is_object()=>{
                    world.commands=catalog.clone();cx.publish_commands("play",&catalog);
                    state.published=pending.generation;state.error=None;
                }
                Ok(_)=>{
                    let message="C# command refresh returned a non-object catalog";
                    state.error=Some(message.into());cx.log(Diagnostic::error(codes::PLAY,message));
                }
                Err(error)=>{
                    let message=error["message"].as_str().unwrap_or("C# command refresh failed");
                    state.error=Some(message.into());cx.log(Diagnostic::error(codes::PLAY,message));
                }
            }
            false
        });
    }
}
fn request(world:&mut World)->Result<u64,Vec<Diagnostic>> {
    if world.fault.is_some() {return Err(fail(codes::PLAYER_FAULT,"C# command authority is faulted"))}
    let (_,player)=world.players.first_mut().ok_or_else(||fail(codes::PLAY,"C# command authority was removed"))?;
    player.request(method::COMMANDS,&json!({})).map_err(|fault|fail(codes::PLAY,fault["message"].as_str().unwrap_or("Cannot request C# commands")))
}
impl PlayService {
    pub(crate) fn queue_catalog_refresh(&mut self,id:&str)->Result<(),Vec<Diagnostic>> {
        let world=self.worlds.get_mut(id).ok_or_else(||fail(codes::UNKNOWN_WORLD,format!("no play world `{id}`")))?;
        self.catalog_refresh.request(id,world)
    }
    pub(crate) fn poll_catalog_refresh(&mut self,cx:&mut Cx<'_>) {
        self.catalog_refresh.poll(&mut self.worlds,cx);
    }
}
