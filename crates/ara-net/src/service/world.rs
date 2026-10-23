use super::*;

/// Fast multi-world runner with real encrypted loopback links (no renderer).
/// Factories/configuration run during start; pre-warm expensive game setup in the caller.
pub struct WorldControl<F: SimulationFeature> {
    factory: fn() -> Result<Session>,
    configure: fn(&mut Server<F>) -> Result<()>,
    policies: PolicyRegistry,
    server: Option<Server<F>>,
    clients: Vec<(PlayerId, Client<F>, Link)>,
    fault: Option<String>,
}
impl<F: SimulationFeature> WorldControl<F> {
    /// Creates an idle runner from the project's world factory and ownership setup.
    pub fn new(
        policies: PolicyRegistry,
        factory: fn() -> Result<Session>,
        configure: fn(&mut Server<F>) -> Result<()>,
    ) -> Self {
        Self {
            factory,
            configure,
            policies,
            server: None,
            clients: Vec::new(),
            fault: None,
        }
    }
}
impl<F: SimulationFeature> NetControl for WorldControl<F> {
    fn start(&mut self, mode: SessionMode, clients: u16, seed: u64) -> Result<Value> {
        if self.server.is_some() || clients == 0 || clients > 16 || mode == SessionMode::Local {
            return Err(NetError(
                "net.start requires an idle Host/Dedicated runner and 1..16 clients".into(),
            ));
        }
        let mut links = (0..clients)
            .map(|n| Link::new(seed.wrapping_add(u64::from(n) * 997), Conditions::default()))
            .collect::<Result<Vec<_>>>()?;
        let addresses = links
            .iter()
            .map(Link::address)
            .collect::<Result<Vec<_>>>()?;
        let mut server = Server::<F>::new(
            (self.factory)()?,
            self.policies.clone(),
            ServerConfig {
                mode,
                start_clients: usize::from(clients),
                max_clients: usize::from(clients),
                expected_addresses: addresses.clone(),
                ..Default::default()
            },
        )?;
        (self.configure)(&mut server)?;
        let address = server.address()?;
        let mut owners = Vec::new();
        for (n, link) in links.iter_mut().enumerate() {
            let player = PlayerId(n as u64 + u64::from(mode == SessionMode::Listen));
            link.set_server(address);
            let endpoint = addresses[n];
            let token = server.token(player, endpoint)?;
            let client = Client::<F>::new(
                self.policies.clone(),
                ClientConfig {
                    player,
                    endpoint,
                    hz: 60,
                    tick_limit: None,
                },
                token,
            )?;
            owners.push((player, client));
        }
        self.clients = owners
            .into_iter()
            .zip(links)
            .map(|((p, c), l)| (p, c, l))
            .collect();
        self.server = Some(server);
        self.fault = None;
        self.status()
    }
    fn stop(&mut self) -> Result<Value> {
        self.clients.clear();
        self.server = None;
        Ok(json!({"stopped":true}))
    }
    fn status(&mut self) -> Result<Value> {
        let clients: Vec<_> = self
            .clients
            .iter_mut()
            .map(|(p, c, l)| json!({"player":p.0,"seed":l.seed(),"status":c.status()}))
            .collect();
        Ok(
            json!({"running":self.server.is_some(),"tick":self.server.as_ref().map(|s|s.session().tick()),"clients":clients,"error":self.fault}),
        )
    }
    fn conditions(&mut self, player: PlayerId, up: Conditions, down: Conditions) -> Result<Value> {
        let (_, _, link) = self
            .clients
            .iter_mut()
            .find(|(p, _, _)| *p == player)
            .ok_or_else(|| NetError("unknown client".into()))?;
        link.conditions(up, down)?;
        Ok(json!({"player":player.0,"seed":link.seed(),"up":up,"down":down}))
    }
    fn profile(&mut self) -> Result<Value> {
        Ok(
            json!({"fields":self.server.as_ref().map(Server::profile),"transport":self.clients.iter().map(|(p,_,l)|json!({"player":p.0,"up":l.up,"down":l.down})).collect::<Vec<_>>() }),
        )
    }
    fn bot(
        &mut self,
        player: PlayerId,
        tick: u64,
        action: String,
        value: ActionValue,
        hold: u32,
    ) -> Result<Value> {
        let input = Injection {
            player,
            action,
            value,
            hold_ticks: hold,
        };
        if let Some((_, client, _)) = self.clients.iter_mut().find(|(p, _, _)| *p == player) {
            client.inject(tick, input)?;
        } else if player == PlayerId(0) {
            self.server
                .as_mut()
                .ok_or_else(|| NetError("no running session".into()))?
                .inject_host(tick, input)?;
        } else {
            return Err(NetError("unknown bot owner".into()));
        }
        Ok(json!({"queued":true}))
    }
    fn poll(&mut self) -> Result<()> {
        if self.fault.is_some() {
            return Ok(());
        }
        let result = (|| {
            let now = Instant::now();
            for (_, _, link) in &mut self.clients {
                link.pump(now, false)?;
            }
            if let Some(server) = &mut self.server {
                server.poll()?;
            }
            for (_, client, link) in &mut self.clients {
                client.poll()?;
                link.pump(now, false)?;
            }
            Ok(())
        })();
        if let Err(e) = &result {
            self.fault = Some(format!("{e}"));
        }
        result
    }
}
