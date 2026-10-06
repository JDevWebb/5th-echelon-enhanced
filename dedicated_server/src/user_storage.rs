use quazal::prudp::ClientRegistry;
use quazal::rmc::types::DateTime;
use quazal::rmc::types::QList;
use quazal::rmc::types::Variant;
use quazal::rmc::Error;
use quazal::rmc::Protocol;
use quazal::ClientInfo;
use quazal::Context;
use slog::Logger;

use crate::login_required;
use crate::protocols::user_storage::types::ContentProperty;
use crate::protocols::user_storage::types::UserContent;
use crate::protocols::user_storage::types::UserContentKey;
use crate::protocols::user_storage::types::UserContentURL;
use crate::protocols::user_storage::user_storage_protocol::GetContentUrlRequest;
use crate::protocols::user_storage::user_storage_protocol::GetContentUrlResponse;
use crate::protocols::user_storage::user_storage_protocol::SaveContentAndGetUploadInfoRequest;
use crate::protocols::user_storage::user_storage_protocol::SaveContentAndGetUploadInfoResponse;
use crate::protocols::user_storage::user_storage_protocol::SearchContentsRequest;
use crate::protocols::user_storage::user_storage_protocol::SearchContentsResponse;
use crate::protocols::user_storage::user_storage_protocol::UploadEndRequest;
use crate::protocols::user_storage::user_storage_protocol::UploadEndResponse;
use crate::protocols::user_storage::user_storage_protocol::UserStorageProtocolServer;
use crate::protocols::user_storage::user_storage_protocol::UserStorageProtocolServerTrait;
use crate::storage::Storage;

/// Implementation of the `UserStorageProtocolServerTrait` for handling user storage requests.
struct UserStorageProtocolServerImpl {
    storage: std::sync::Arc<Storage>,
}

impl<CI> UserStorageProtocolServerTrait<CI> for UserStorageProtocolServerImpl {
    /// Handles the `SearchContents` request, returning a list of user content.
    ///
    /// This function requires the client to be logged in. It currently returns a hardcoded
    /// list of user content for a specific query type.
    fn search_contents(
        &self,
        _logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SearchContentsRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SearchContentsResponse, Error> {
        #![allow(clippy::unreadable_literal)]

        let user_id = login_required(&*ci)?;
        // The player's own ShadowNet snapshot, if there's one: the game overwrites it then,
        // instead of adding one (uploads.rs). Only ever the caller's.
        if request.query.type_id == crate::uploads::SHADOWNET {
            let mine = crate::storage::run(self.storage.has_content(user_id, crate::uploads::SHADOWNET))
                .ok()
                .and_then(Result::ok)
                .unwrap_or(false);
            let search_results = if mine {
                QList(vec![UserContent {
                    key: UserContentKey {
                        type_id: crate::uploads::SHADOWNET,
                        content_id: u64::from(user_id),
                    },
                    pid: user_id,
                    properties: QList::default(),
                }])
            } else {
                QList::default()
            };
            return Ok(SearchContentsResponse { search_results });
        }
        if request.query.type_id == 0x8000_0002 {
            let search_results = QList(vec![UserContent {
                key: UserContentKey {
                    type_id: 0x8000_0002,
                    content_id: 1,
                },
                pid: 0x0000_045f,
                properties: QList(vec![
                    ContentProperty {
                        id: 6,
                        value: Variant::I64(0x274),
                    },
                    ContentProperty {
                        id: 4,
                        value: Variant::DateTime(DateTime(0x1f768edbd6)),
                    },
                    ContentProperty {
                        id: 5,
                        value: Variant::DateTime(DateTime(0x1f768f13f9)),
                    },
                    ContentProperty {
                        id: 7,
                        value: Variant::String("A6E32CFD0C2B2CFFE2D0C785830B7C49".to_string()),
                    },
                ]),
            }]);
            Ok(SearchContentsResponse { search_results })
        } else {
            Ok(SearchContentsResponse { search_results: QList::default() })
        }
    }

    /// The game asking where to upload a player's content: for its ShadowNet snapshot, a
    /// one-time address on the content server (uploads.rs); anything else is refused.
    fn save_content_and_get_upload_info(
        &self,
        logger: &Logger,
        ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: SaveContentAndGetUploadInfoRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<SaveContentAndGetUploadInfoResponse, Error> {
        let user_id = login_required(&*ci)?;
        let Some(host) = ctx.settings.get("storage_host").cloned() else {
            return Err(Error::AccessDenied);
        };
        match crate::uploads::begin(user_id, request.content_key.type_id, request.size) {
            Ok((pending_id, secret)) => {
                info!(logger, "User {user_id} uploads {} bytes of content type {:#x}", request.size, request.content_key.type_id);
                Ok(SaveContentAndGetUploadInfoResponse {
                    upload_info: UserContentURL {
                        protocol: ctx.settings.get("content_protocol").map_or("http://", String::as_str).to_owned(),
                        host,
                        path: crate::uploads::path(pending_id, &secret),
                    },
                    pending_id,
                    headers: vec![],
                })
            }
            Err(why) => {
                info!(
                    logger,
                    "User {user_id}'s upload of {} bytes of content type {:#x} refused ({why:?})", request.size, request.content_key.type_id
                );
                Err(Error::AccessDenied)
            }
        }
    }

    /// The game says how its upload went: kept when it arrived (the player's latest, for the
    /// admins), with the key the game's next search finds.
    fn upload_end(
        &self,
        logger: &Logger,
        _ctx: &Context,
        ci: &mut ClientInfo<CI>,
        request: UploadEndRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<UploadEndResponse, Error> {
        use std::io::Write as _;
        let user_id = login_required(&*ci)?;
        let Some(body) = crate::uploads::finish(user_id, request.pending_id, request.result) else {
            info!(logger, "User {user_id}'s upload {} didn't arrive (it says {})", request.pending_id, request.result);
            return Err(Error::AccessDenied);
        };
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let gzip = gz.write_all(&body).and_then(|()| gz.finish()).map_err(|_| Error::InternalError)?;
        if let Err(e) = crate::storage::run(self.storage.save_content(user_id, crate::uploads::SHADOWNET, &gzip, body.len())).and_then(|r| r) {
            slog::error!(logger, "keeping {user_id}'s upload failed: {e}");
            return Err(Error::InternalError);
        }
        info!(logger, "User {user_id}'s ShadowNet snapshot kept ({} KB, {} KB gzip)", body.len() / 1024, gzip.len() / 1024);
        crate::federation::report_queued();
        Ok(UploadEndResponse {
            content_key: UserContentKey {
                type_id: crate::uploads::SHADOWNET,
                content_id: u64::from(user_id),
            },
        })
    }

    /// Handles the `GetContentUrl` request, returning the URL for a piece of user content.
    ///
    /// This function requires the client to be logged in. It constructs the URL from
    /// the server's configuration.
    fn get_content_url(
        &self,
        _logger: &Logger,
        ctx: &Context,
        ci: &mut ClientInfo<CI>,
        _request: GetContentUrlRequest,
        _client_registry: &ClientRegistry<CI>,
        _socket: &std::net::UdpSocket,
    ) -> Result<GetContentUrlResponse, Error> {
        login_required(&*ci)?;
        let protocol = ctx.settings.get("content_protocol").map_or("http://", String::as_str).to_owned();
        let host = ctx.settings.get("storage_host").expect("missing storage_host setting").to_owned();
        let path = ctx.settings.get("storage_path").expect("missing storage_path setting").to_owned();

        Ok(GetContentUrlResponse {
            download_info: UserContentURL { protocol, host, path },
        })
    }
}

/// Creates a new boxed `UserStorageProtocolServer` instance.
///
/// This function is typically used to register the user storage protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>(storage: std::sync::Arc<Storage>) -> Box<dyn Protocol<T>> {
    Box::new(UserStorageProtocolServer::new(UserStorageProtocolServerImpl { storage }))
}
