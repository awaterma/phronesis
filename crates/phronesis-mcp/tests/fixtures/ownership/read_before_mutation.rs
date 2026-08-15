// Core case 3: a function that reads/snapshots something rooted at a place,
// then mutates through the same root place.
// Per D4 the root place of self.party.members[i].pos is `self`.

struct Member {
    name: String,
    pos: i32,
}

struct Party {
    members: Vec<Member>,
}

struct State {
    party: Party,
}

impl State {
    fn snapshot_and_mutate(&mut self) {
        let snapshot = self.party.members.clone();
        let _orig = self.party.members[0].pos;
        let _mut_ref = self.party.members.get_mut(0);
    }
}
