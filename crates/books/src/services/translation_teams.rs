//! A-04: owner-scoped Team commands; membership never grants Book access.
#![forbid(unsafe_code)]
use crate::repositories::translation_teams::{validate_team, PgTranslationTeams};
pub use crate::repositories::translation_teams::{TeamError, TranslationTeam};

#[derive(Clone)]
pub struct TeamService {
    repository: PgTranslationTeams,
}

impl TeamService {
    pub fn new(repository: PgTranslationTeams) -> Self {
        Self { repository }
    }

    /// The HTTP boundary supplies the verified session owner exactly once.
    pub fn for_owner(&self, actor: i64) -> Result<OwnedTeamService<'_>, TeamError> {
        check_actor(actor)?;
        Ok(OwnedTeamService {
            repository: &self.repository,
            actor,
        })
    }
}

fn check_actor(actor: i64) -> Result<(), TeamError> {
    if actor <= 0 {
        Err(TeamError::Missing)
    } else {
        Ok(())
    }
}

pub struct OwnedTeamService<'a> {
    repository: &'a PgTranslationTeams,
    actor: i64,
}

impl OwnedTeamService<'_> {
    pub async fn list(&self) -> Result<Vec<TranslationTeam>, TeamError> {
        self.repository.list_owned(self.actor).await
    }

    pub async fn create(
        &self,
        name: &str,
        description: &str,
    ) -> Result<TranslationTeam, TeamError> {
        // Service handles input rules; the PG adapter independently rechecks
        // the active owner in the same transaction as INSERT.
        let (name, description) = validate_team(name, description)?;
        self.repository.create(self.actor, &name, &description).await
    }

    pub async fn get(&self, id: i64) -> Result<TranslationTeam, TeamError> {
        self.repository.owned(self.actor, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a04_rejects_invalid_owner_without_database() {
        assert_eq!(check_actor(0), Err(TeamError::Missing));
        assert_eq!(check_actor(-1), Err(TeamError::Missing));
        assert_eq!(check_actor(7), Ok(()));
    }

    #[test]
    fn a04_service_reuses_team_validation_before_create() {
        assert_eq!(validate_team(" \n", ""), Err(TeamError::Invalid));
        assert_eq!(
            validate_team(" Team ", " Description "),
            Ok(("Team".to_owned(), "Description".to_owned()))
        );
    }
}
