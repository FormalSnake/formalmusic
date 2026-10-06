//! Shared daemon state and the request dispatch.

use crate::config::{Config, Paths};
use crate::extras::Extras;
use crate::playback::Playback;
use crate::session::Session;
use crate::signin::BrowserSignIn;
use formalmusic_api::{ApiError, Command, Event, LibraryScope, RateTarget, Reply};
use formalmusic_player::Player;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct Daemon {
    pub session: Arc<Session>,
    pub playback: Arc<Playback>,
    extras: Arc<Extras>,
    signin: BrowserSignIn,
    pub events: broadcast::Sender<Event>,
}

impl Daemon {
    pub fn new(paths: &Paths, config: Config, player: Player) -> anyhow::Result<Arc<Self>> {
        crate::config::create_private_dir(&paths.state)?;
        let session = Arc::new(Session::load(paths.session())?);
        let (events, _) = broadcast::channel(256);
        let playback = Playback::new(player, session.clone(), config, paths, events.clone())?;
        let extras = Arc::new(Extras::new()?);
        tokio::spawn(crate::extras::warm_on_track_change(
            extras.clone(),
            session.clone(),
            playback.clone(),
            events.subscribe(),
        ));
        Ok(Arc::new(Self {
            session,
            playback,
            extras,
            signin: BrowserSignIn::new(paths.signin()),
            events,
        }))
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn session_changed(&self) {
        self.playback.session_changed();
        self.emit(Event::Session(self.session.info()));
    }

    fn library_changed(&self, scope: LibraryScope) -> Result<Reply, ApiError> {
        self.emit(Event::LibraryChanged { scope });
        Ok(Reply::Ok)
    }

    /// Checks the stored cookies once at startup.
    pub async fn check_session(&self) {
        if self.session.cookies().is_none() {
            return;
        }
        match self.session.refresh().await {
            Ok(info) => {
                tracing::info!(
                    signed_in = info.signed_in,
                    premium = info.premium,
                    "session checked"
                );
                self.emit(Event::Session(info));
            }
            Err(e) => tracing::warn!("could not check the stored session: {e}"),
        }
    }

    pub async fn dispatch(&self, command: Command) -> Result<Reply, ApiError> {
        let client = self.session.client();
        let playback = &self.playback;
        match command {
            Command::Session => Ok(Reply::Session(self.session.info())),
            Command::SignIn { cookies } => {
                let info = self.session.sign_in(&cookies, None).await?;
                self.session_changed();
                Ok(Reply::Session(info))
            }
            Command::Browsers => Ok(Reply::Browsers(self.signin.browsers().await)),
            Command::BrowserSignIn { browser } => {
                let cookies = self.signin.run(browser.as_deref()).await?;
                let info = self.session.sign_in(&cookies, None).await?;
                self.session_changed();
                Ok(Reply::Session(info))
            }
            Command::CancelSignIn => {
                self.signin.cancel();
                Ok(Reply::Ok)
            }
            Command::SignOut => {
                let info = self.session.sign_out()?;
                self.session_changed();
                Ok(Reply::Session(info))
            }
            Command::Accounts => Ok(Reply::Accounts(client.accounts().await?)),
            Command::SwitchAccount { page_id } => {
                let info = self.session.switch_account(page_id).await?;
                self.session_changed();
                Ok(Reply::Session(info))
            }

            Command::Browse { target } => Ok(Reply::Page(client.browse(target).await?)),
            Command::Continue { token } => {
                Ok(Reply::Continuation(client.continuation(&token).await?))
            }
            Command::Search { query, filter } => {
                Ok(Reply::Search(client.search(&query, filter).await?))
            }
            Command::Suggestions { query } => {
                Ok(Reply::Suggestions(client.suggestions(&query).await?))
            }
            Command::Lyrics { video_id } => {
                let known = playback.queued_track(&video_id);
                Ok(Reply::Lyrics(
                    self.extras.lyrics(&client, &video_id, known).await?,
                ))
            }
            Command::AnimatedCover { artist, album } => Ok(Reply::AnimatedCover(
                self.extras.animated_cover(&artist, &album).await?,
            )),
            Command::Related { browse_id } => Ok(Reply::Page(client.related(&browse_id).await?)),

            Command::Rate { target, rating } => {
                client.rate(&target, rating).await?;
                self.library_changed(match target {
                    RateTarget::Track { .. } => LibraryScope::Likes,
                    RateTarget::Playlist { .. } => LibraryScope::Playlists,
                })
            }
            Command::SetSubscribed {
                channel_id,
                subscribed,
            } => {
                client.set_subscribed(&channel_id, subscribed).await?;
                self.library_changed(LibraryScope::Subscriptions)
            }
            Command::CreatePlaylist {
                title,
                description,
                privacy,
                video_ids,
            } => {
                let playlist_id = client
                    .create_playlist(&title, &description, privacy, &video_ids)
                    .await?;
                self.emit(Event::LibraryChanged {
                    scope: LibraryScope::Playlists,
                });
                Ok(Reply::PlaylistCreated { playlist_id })
            }
            Command::EditPlaylist { playlist_id, edits } => {
                client.edit_playlist(&playlist_id, &edits).await?;
                self.library_changed(LibraryScope::Playlists)
            }
            Command::DeletePlaylist { playlist_id } => {
                client.delete_playlist(&playlist_id).await?;
                self.library_changed(LibraryScope::Playlists)
            }
            Command::SetInLibrary { playlist_id, saved } => {
                client.set_in_library(&playlist_id, saved).await?;
                self.library_changed(if playlist_id.starts_with("OLAK") {
                    LibraryScope::Albums
                } else {
                    LibraryScope::Playlists
                })
            }
            Command::RemoveFromHistory { feedback_token } => {
                client.remove_from_history(&feedback_token).await?;
                self.library_changed(LibraryScope::History)
            }

            Command::Play {
                source,
                start_index,
                shuffle,
                radio,
            } => {
                playback.play(source, start_index, shuffle, radio).await?;
                Ok(Reply::Ok)
            }
            Command::Enqueue { tracks, position } => {
                playback.enqueue(tracks, position);
                Ok(Reply::Ok)
            }
            Command::RemoveFromQueue { index } => playback.remove(index).map(|()| Reply::Ok),
            Command::MoveInQueue { from, to } => playback.move_entry(from, to).map(|()| Reply::Ok),
            Command::ClearQueue => {
                playback.clear();
                Ok(Reply::Ok)
            }
            Command::JumpTo { index } => playback.jump(index).map(|()| Reply::Ok),
            Command::Toggle => {
                playback.toggle();
                Ok(Reply::Ok)
            }
            Command::Pause => {
                playback.pause();
                Ok(Reply::Ok)
            }
            Command::Resume => {
                playback.resume();
                Ok(Reply::Ok)
            }
            Command::Next => {
                playback.next();
                Ok(Reply::Ok)
            }
            Command::Previous => {
                playback.previous();
                Ok(Reply::Ok)
            }
            Command::SeekTo { position_ms } => {
                playback.seek(position_ms);
                Ok(Reply::Ok)
            }
            Command::SetVolume { volume } => {
                playback.set_volume(volume);
                Ok(Reply::Ok)
            }
            Command::SetMuted { muted } => {
                playback.set_muted(muted);
                Ok(Reply::Ok)
            }
            Command::SetRepeat { repeat } => {
                playback.set_repeat(repeat);
                Ok(Reply::Ok)
            }
            Command::SetShuffle { shuffle } => {
                playback.set_shuffle(shuffle);
                Ok(Reply::Ok)
            }
            Command::PlayerState => Ok(Reply::Player(playback.player_state())),
            Command::QueueState => Ok(Reply::Queue(playback.queue_state())),

            // The connection handles these itself.
            Command::Hello { .. } | Command::Subscribe => {
                Err(ApiError::BadRequest("not a dispatched command".into()))
            }
        }
    }
}
