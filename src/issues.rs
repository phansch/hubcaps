use std::io::Result;
use self::super::Github;
use rep::{Issue, IssueReq, Label};
use rustc_serialize::json;

pub struct IssueRef<'a> {
  github: &'a Github<'a>,
  owner: &'static str,
  repo: &'static str,
  number: &'static i64
}

impl<'a> IssueRef<'a> {
  /// create a new instance of a github repo issue ref
  pub fn new(
    github: &'a Github<'a>, owner: &'static str, repo: &'static str,
    number: &'static i64) -> IssueRef<'a> {
    IssueRef {
      github: github,
      owner: owner,
      repo: repo,
      number: number
    }
  }

  fn path(&self, more: &str) -> String {
    format!("/repos/{}/{}/issues/{}{}", self.owner, self.repo, self.number, more)
  }

  // add a set of labels to this issue ref
  pub fn label(&self, labels: Vec<&str>) -> Result<Vec<Label>> {
    let body = try!(self.github.post(
      &self.path("/labels"),
      json::encode(&labels).unwrap().as_bytes()
    ));
    Ok(json::decode::<Vec<Label>>(&body).unwrap())
  }
}

pub struct Issues<'a> {
  github: &'a Github<'a>,
  owner: &'static str,
  repo: &'static str
}

impl<'a> Issues<'a> {
  /// create a new instance of a github repo issue ref
  pub fn new(
    github: &'a Github<'a>, owner: &'static str, repo: &'static str) -> Issues<'a> {
    Issues {
      github: github,
      owner: owner,
      repo: repo
    }
  }

  fn path(&self, more: &str) -> String {
    format!("/repos/{}/{}/issues/{}", self.owner, self.repo, more)
  }

  pub fn create(&self, is: &IssueReq) -> Result<Issue> {
    let data = json::encode(&is).unwrap();
    let body = try!(
      self.github.post(
        &self.path(""),
        data.as_bytes()
      )
    );
    Ok(json::decode::<Issue>(&body).unwrap())
  }
}